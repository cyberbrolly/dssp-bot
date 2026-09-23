"""Playwright-backed DSSP portal client.

Owns a persistent Chromium context. The operator logs in once in the opened
window; the session is stored on disk (python/.pw-profile) and reused. Data
operations run through the context's request API so they share the login
cookies, mirroring the old extension's credentialed fetch approach.

This task implements the context lifecycle and the manual-login session gate.
Read/submit operations are added in later tasks.
"""

from __future__ import annotations

import logging
import time
from pathlib import Path
from typing import Optional
from urllib.parse import urlencode, urljoin

from playwright.sync_api import Page, Playwright, sync_playwright
from playwright.sync_api import Error as PlaywrightError

from . import constants as c
from . import errors as e
from . import parsing as parse

log = logging.getLogger("dssp.portal")


class PortalClient:
    def __init__(self) -> None:
        self._pw: Optional[Playwright] = None
        self._context = None  # BrowserContext (persistent)

    # -- lifecycle ----------------------------------------------------------
    def start(self) -> None:
        if self._context is not None:
            return

        Path(c.PROFILE_DIR).mkdir(parents=True, exist_ok=True)
        self._pw = sync_playwright().start()
        self._context = self._pw.chromium.launch_persistent_context(
            c.PROFILE_DIR,
            headless=c.HEADLESS,
            args=["--no-first-run", "--no-default-browser-check"],
        )
        log.info(
            "browser context started (headless=%s, profile=%s)",
            c.HEADLESS,
            c.PROFILE_DIR,
        )

    def close(self) -> None:
        try:
            if self._context is not None:
                self._context.close()
        except Exception:  # noqa: BLE001 - best-effort teardown
            log.exception("error closing browser context")
        finally:
            if self._pw is not None:
                self._pw.stop()
            self._context = None
            self._pw = None

    def _page(self) -> Page:
        self.start()
        assert self._context is not None
        pages = self._context.pages
        return pages[0] if pages else self._context.new_page()

    # -- session gate -------------------------------------------------------
    def ensure_session(self, timeout_ms: Optional[int] = None) -> bool:
        """Open /Trainee and, if the portal shows the login page, wait until the
        operator signs in. Returns True once an authenticated page is detected;
        raises SESSION_EXPIRED if the timeout elapses first."""
        timeout_ms = c.LOGIN_TIMEOUT_MS if timeout_ms is None else timeout_ms
        page = self._page()

        try:
            page.goto(
                c.TRAINEE_PAGE_URL,
                wait_until="domcontentloaded",
                timeout=c.NAV_TIMEOUT_MS,
            )
        except PlaywrightError as exc:
            raise e.network(f"could not open {c.TRAINEE_PAGE_URL}: {exc}") from exc

        deadline = time.monotonic() + timeout_ms / 1000.0
        announced = False

        while True:
            soup = parse.parse_html(page.content())
            href = page.url

            if not parse.is_login_page(soup, href) and parse.has_authenticated_marker(
                soup
            ):
                log.info("authenticated DSSP session detected at %s", href)
                return True

            if parse.is_login_page(soup, href) and not announced:
                log.warning(
                    "login required — sign in to DSSP in the opened browser window"
                )
                announced = True

            if time.monotonic() >= deadline:
                raise e.session_expired()

            time.sleep(1.0)

    # -- reads --------------------------------------------------------------
    def _get(self, url: str, evidence: str = ""):
        """GET through the context request API so it carries the login cookies.

        Returns (soup, final_url, body, status). Raises SESSION_EXPIRED when the
        portal answers with the login page, mirroring the old loadDocument.

        `evidence` names the raw body for a live-gate dump (see `_dump`); empty
        means this read is not dumped."""
        self.start()
        assert self._context is not None
        try:
            resp = self._context.request.get(url, timeout=c.NAV_TIMEOUT_MS)
            body = resp.text()
        except PlaywrightError as exc:
            raise e.network(f"GET {url} failed: {exc}") from exc

        final_url = resp.url
        # Dumped before the checks below, not after: a login page or an error
        # body is exactly what an operator needs to see when the run fails.
        if evidence:
            self._dump(evidence, body, final_url)
        soup = parse.parse_html(body)

        # Login check comes first: a lapsed session often 200s to the login page.
        if parse.is_login_page(soup, final_url):
            raise e.session_expired()

        if not resp.ok:
            raise e.network(f"DSSP returned HTTP {resp.status} for {url}")

        return soup, final_url, body, resp.status

    def _dump(self, name: str, body: str, source: str) -> None:
        """Write a raw portal response to `DSSP_DUMP_DIR`, when set.

        Off unless an operator asks for it, and never load-bearing: a dump that
        fails logs and returns, because evidence gathering must not be able to
        fail a submission.

        The body is the portal's own bytes, unredacted — it can carry trainee
        data and, on a form, the antiforgery token. It therefore goes to a
        gitignored directory, never to stdout (protocol lines only) and never
        into the log: the log gets the path, not the content.
        """
        target = c.dump_dir()
        if not target:
            return

        try:
            directory = Path(target)
            directory.mkdir(parents=True, exist_ok=True)
            path = directory / name
            # errors="replace": a body that is not valid UTF-8 (a lone surrogate
            # from a mislabelled charset) must still be dumped rather than
            # raising — UnicodeEncodeError is a ValueError, not an OSError, so it
            # would slip past the handler below and fail the submission.
            path.write_text(body, encoding="utf-8", errors="replace")
        except (OSError, ValueError) as exc:
            log.warning("could not dump %s: %s", source, exc)
            return

        log.info("dumped %s → %s", source, path)

    def list_trainees(self) -> list[dict]:
        soup, _url, _body, _status = self._get(
            c.TRAINEE_LIST_URL, evidence="trainee-list.html"
        )
        return parse.get_trainees(soup)

    def get_form_options(self, trainee_id: Optional[str] = None) -> dict:
        """Load the training form and scrape its instructor/type options.

        With no trainee_id, uses the first trainee — the form is the same for
        every trainee, so any one exposes the option lists."""
        if not trainee_id:
            trainees = self.list_trainees()
            if not trainees:
                raise e.missing_data("a trainee to load form options")
            trainee_id = trainees[0]["id"]

        soup, _url, _body, _status = self._get(
            c.training_form_url(trainee_id),
            evidence=f"training-form-{trainee_id}.html",
        )
        return {"trainee_id": trainee_id, **parse.get_form_options(soup)}

    # -- submit -------------------------------------------------------------
    def _resolve_trainee(self, trainee: dict) -> dict:
        """Resolve a trainee safely. By id when given, else by normalized name.
        Never guesses: an ambiguous name match stops the job."""
        trainee_id = str(trainee.get("id") or "").strip()
        if trainee_id:
            for candidate in self.list_trainees():
                if candidate["id"] == trainee_id:
                    return candidate
            raise e.trainee_not_found(
                f"No trainee with id {trainee_id} on the portal."
            )

        name = str(trainee.get("name") or "").strip()
        if not name:
            raise e.missing_data("trainee id or name")

        target = parse.normalize_name(name)
        matches = [
            t
            for t in self.list_trainees()
            if parse.normalize_name(t["name"]) == target
        ]
        if len(matches) == 1:
            return matches[0]
        if not matches:
            raise e.trainee_not_found(f"No trainee named {name!r} on the portal.")
        raise e.trainee_not_found(
            f"Ambiguous trainee match: {len(matches)} trainees named {name!r}. "
            "Provide the trainee id."
        )

    def submit_training(self, trainee: dict, session: dict) -> dict:
        """Prepare, commit exactly once, then classify. Never re-POST."""
        resolved = self._resolve_trainee(trainee)

        soup, form_url, _body, _status = self._get(
            c.training_form_url(resolved["id"]),
            evidence=f"training-form-{resolved['id']}.html",
        )
        form, payload = parse.build_form_payload(soup, session)

        action = urljoin(form_url, form.get("action") or form_url)
        data = urlencode(payload)

        self.start()
        assert self._context is not None
        try:
            resp = self._context.request.post(
                action,
                data=data,
                headers={
                    "Content-Type": "application/x-www-form-urlencoded",
                    "X-Requested-With": "XMLHttpRequest",
                },
                timeout=c.NAV_TIMEOUT_MS,
            )
        except PlaywrightError as exc:
            raise e.network(
                f"submitting training for trainee {resolved['id']} failed: {exc}"
            ) from exc

        body = resp.text()
        # The response to a real POST is the one piece of evidence worth having:
        # "duplicate" is a text match on this body (see submission_outcome), and
        # the crash-window safety argument leans on that match being right.
        self._dump(f"submit-response-{resolved['id']}.html", body, resp.url)
        redirected = resp.url != action
        outcome = parse.submission_outcome(body, resp.url, resp.status, redirected)

        log.info(
            "submit trainee=%s name=%r outcome=%s",
            resolved["id"],
            resolved["name"],
            outcome.get("outcome"),
        )
        return {
            "trainee": {"id": resolved["id"], "name": resolved["name"]},
            "attempts": 1,
            **outcome,
        }
