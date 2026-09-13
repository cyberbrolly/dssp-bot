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
