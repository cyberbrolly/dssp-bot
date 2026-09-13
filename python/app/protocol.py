"""Wire protocol for the DSSP-Bot worker.

One JSON object per line. stdout carries protocol lines only; every log line
goes to stderr. ``job_id`` is echoed on every response so Rust can pair a reply
with its request.
"""

from __future__ import annotations

import json
from typing import Any, TextIO

V = 1

# Operations ----------------------------------------------------------------
OP_PING = "ping"
OP_ENSURE_SESSION = "ensure_session"
OP_LIST_TRAINEES = "list_trainees"
OP_GET_FORM_OPTIONS = "get_form_options"
OP_SUBMIT_TRAINING = "submit_training"

# Error codes ---------------------------------------------------------------
# Mirror of src/core/shared/errors.ts ErrorCode, plus protocol-level codes.
ERR_ELEMENT_NOT_FOUND = "ELEMENT_NOT_FOUND"
ERR_TIMEOUT = "TIMEOUT"
ERR_NETWORK = "NETWORK"
ERR_PORTAL_UNAVAILABLE = "PORTAL_UNAVAILABLE"
ERR_SESSION_EXPIRED = "SESSION_EXPIRED"
ERR_TRAINEE_NOT_FOUND = "TRAINEE_NOT_FOUND"
ERR_MISSING_DATA = "MISSING_DATA"
ERR_VALIDATION_FAILED = "VALIDATION_FAILED"
ERR_DUPLICATE_RECORD = "DUPLICATE_RECORD"
ERR_PORTAL_STRUCTURE_CHANGED = "PORTAL_STRUCTURE_CHANGED"
ERR_PORTAL_NOT_MAPPED = "PORTAL_NOT_MAPPED"
ERR_CONFIRMATION_UNKNOWN = "CONFIRMATION_UNKNOWN"
ERR_SUBMISSION_FAILED = "SUBMISSION_FAILED"
# Protocol-level (never reach the portal).
ERR_BAD_REQUEST = "BAD_REQUEST"
ERR_UNKNOWN_OP = "UNKNOWN_OP"

# Codes that prove nothing reached the portal — mirror of provesNothingSubmitted
# in AutomationEngine.ts. Rust owns the retry decision; this is only a hint.
_PROVES_NOTHING = frozenset(
    {
        ERR_ELEMENT_NOT_FOUND,
        ERR_PORTAL_NOT_MAPPED,
        ERR_PORTAL_STRUCTURE_CHANGED,
        ERR_SESSION_EXPIRED,
        ERR_VALIDATION_FAILED,
        ERR_MISSING_DATA,
        ERR_TRAINEE_NOT_FOUND,
        ERR_BAD_REQUEST,
        ERR_UNKNOWN_OP,
    }
)


def proves_nothing_submitted(error_code: str) -> bool:
    """Whether this error was raised before anything could reach the portal."""
    return error_code in _PROVES_NOTHING


def ready_line() -> dict[str, Any]:
    return {"v": V, "status": "ready"}


def ok_response(job_id: str, op: str, **fields: Any) -> dict[str, Any]:
    return {"v": V, "job_id": job_id, "op": op, "status": "ok", **fields}


def error_response(
    job_id: str, op: str, error_code: str, message: str
) -> dict[str, Any]:
    return {
        "v": V,
        "job_id": job_id,
        "op": op,
        "status": "error",
        "error_code": error_code,
        "message": message,
        "proves_nothing_submitted": proves_nothing_submitted(error_code),
    }


def write_line(stream: TextIO, obj: dict[str, Any]) -> None:
    """Emit a single protocol line and flush so Rust sees it immediately."""
    stream.write(json.dumps(obj, separators=(",", ":")) + "\n")
    stream.flush()
