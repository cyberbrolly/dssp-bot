"""DSSP-Bot Python worker.

Reads one JSON request per line from stdin, dispatches it, and writes one JSON
response per line to stdout. All logging goes to stderr so stdout stays a clean
protocol channel. Rust owns retry/stop decisions; this worker only reports.
"""

from __future__ import annotations

import json
import logging
import sys
from typing import Any, Callable

from . import protocol as p
from .portal.client import PortalClient
from .portal.errors import PortalError

log = logging.getLogger("dssp.worker")

Handler = Callable[[dict[str, Any]], dict[str, Any]]


def _configure_logging() -> None:
    logging.basicConfig(
        stream=sys.stderr,
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )


class Worker:
    """Dispatches protocol requests to op handlers.

    Handlers are registered here; later tasks add the portal-backed ops.
    """

    def __init__(self) -> None:
        self.portal = PortalClient()
        self._handlers: dict[str, Handler] = {
            p.OP_PING: self._ping,
            p.OP_ENSURE_SESSION: self._ensure_session,
            p.OP_LIST_TRAINEES: self._list_trainees,
            p.OP_GET_FORM_OPTIONS: self._get_form_options,
        }

    def handle(self, req: dict[str, Any]) -> dict[str, Any]:
        job_id = req.get("job_id", "")
        op = req.get("op", "")

        if not isinstance(op, str) or not op:
            return p.error_response(job_id, "", p.ERR_BAD_REQUEST, "missing 'op'")

        handler = self._handlers.get(op)
        if handler is None:
            return p.error_response(
                job_id, op, p.ERR_UNKNOWN_OP, f"unknown op: {op}"
            )

        try:
            return handler(req)
        except PortalError as exc:
            log.warning("op %s failed: %s (%s)", op, exc.message, exc.error_code)
            return p.error_response(job_id, op, exc.error_code, exc.message)
        except Exception as exc:  # noqa: BLE001 - report, never crash the loop
            log.exception("op %s failed", op)
            return p.error_response(job_id, op, p.ERR_SUBMISSION_FAILED, str(exc))

    def _ping(self, req: dict[str, Any]) -> dict[str, Any]:
        return p.ok_response(req.get("job_id", ""), p.OP_PING, pong=True)

    def _ensure_session(self, req: dict[str, Any]) -> dict[str, Any]:
        self.portal.ensure_session()
        return p.ok_response(
            req.get("job_id", ""), p.OP_ENSURE_SESSION, authenticated=True
        )

    def _list_trainees(self, req: dict[str, Any]) -> dict[str, Any]:
        trainees = self.portal.list_trainees()
        projected = [
            {k: t[k] for k in ("id", "name", "sn", "course", "training_sessions")}
            for t in trainees
        ]
        return p.ok_response(
            req.get("job_id", ""),
            p.OP_LIST_TRAINEES,
            count=len(projected),
            trainees=projected,
        )

    def _get_form_options(self, req: dict[str, Any]) -> dict[str, Any]:
        trainee_id = req.get("trainee_id")
        opts = self.portal.get_form_options(trainee_id)
        return p.ok_response(
            req.get("job_id", ""),
            p.OP_GET_FORM_OPTIONS,
            trainee_id=opts.get("trainee_id"),
            instructors=opts["instructors"],
            training_types=opts["training_types"],
        )

    def close(self) -> None:
        self.portal.close()


def main() -> int:
    _configure_logging()
    worker = Worker()

    # Readiness handshake: the first stdout line Rust waits for.
    p.write_line(sys.stdout, p.ready_line())
    log.info("worker ready")

    try:
        while True:
            line = sys.stdin.readline()
            if not line:  # EOF: Rust closed the pipe.
                break

            line = line.strip()
            if not line:
                continue

            try:
                req = json.loads(line)
                if not isinstance(req, dict):
                    raise ValueError("request must be a JSON object")
            except (ValueError, json.JSONDecodeError) as exc:
                p.write_line(
                    sys.stdout,
                    p.error_response(
                        "", "", p.ERR_BAD_REQUEST, f"invalid JSON: {exc}"
                    ),
                )
                continue

            p.write_line(sys.stdout, worker.handle(req))
    finally:
        worker.close()

    log.info("worker stdin closed; exiting")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
