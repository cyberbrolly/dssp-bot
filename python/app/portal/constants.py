"""Portal constants: URLs, selectors, and runtime configuration.

Ported from src/core/infrastructure/portal/DSSPPortalAdapter.ts. All portal
knowledge lives here so the rest of the worker stays portal-agnostic.
"""

from __future__ import annotations

import os
import re
from pathlib import Path
from urllib.parse import quote


def _flag(name: str, default: str = "0") -> bool:
    return os.environ.get(name, default).strip().lower() in ("1", "true", "yes", "on")


# Origin / URLs -------------------------------------------------------------
DSSP_ORIGIN = os.environ.get("DSSP_ORIGIN", "https://dssp.frsc.gov.ng").rstrip("/")
TRAINEE_PAGE_PATH = "/Trainee"
TRAINING_FORM_PATH = "/Trainee/TrainingLog/TraineeId="

TRAINEE_PAGE_URL = f"{DSSP_ORIGIN}{TRAINEE_PAGE_PATH}"
TRAINEE_LIST_URL = f"{DSSP_ORIGIN}/Trainee?pgsize=10000&page=1&keywords="


def training_form_url(trainee_id: str) -> str:
    return f"{DSSP_ORIGIN}{TRAINING_FORM_PATH}{quote(trainee_id)}"


# Selectors -----------------------------------------------------------------
TRAINEE_ROW_SELECTOR = "table.table-checkable tbody tr"
TRAINEE_ID_LINK_SELECTOR = 'a[href*="TraineeId="]'
TRAINING_LOG_LINK_SELECTOR = 'a[href*="/Trainee/TrainingLog/"][href*="TraineeId="]'
TRAINEE_TABLE_SELECTOR = "table.table-checkable tbody"
LOGIN_FORM_SELECTOR = '#loginForm, form[action*="/Account/Login"]'
VALIDATION_MESSAGE_SELECTOR = ".validation-summary-errors, .field-validation-error"
SUBMIT_SELECTOR = 'button[type="submit"], input[type="submit"]'
LOGOUT_SELECTOR = 'a[href*="/Account/Logout"], form[action*="/Account/Logout"]'

TRAINING_DATE_SELECTORS = (
    "#TrainingDate",
    'input[name="TrainingDate"]',
    "#Date",
    'input[name="Date"]',
)
INSTRUCTOR_SELECTORS = (
    "select#Instructor",
    'select[name="Instructor"]',
    "select#InstructorId",
    'select[name="InstructorId"]',
    "select#InstructorID",
    'select[name="InstructorID"]',
)
TRAINING_TYPE_SELECTORS = (
    "select#TrainingType",
    'select[name="TrainingType"]',
    "select#TrainingTypeId",
    'select[name="TrainingTypeId"]',
    "select#TrainingTypeID",
    'select[name="TrainingTypeID"]',
)

# TraineeId=123 in an href, preceded by ? & or / (case-insensitive).
TRAINEE_ID_RE = re.compile(r"(?:[?&/])TraineeId=(\d+)", re.IGNORECASE)

# Runtime configuration -----------------------------------------------------
# Persistent Chromium profile lives under python/.pw-profile (gitignored). The
# operator logs in once and the session survives across worker restarts.
_PYTHON_ROOT = Path(__file__).resolve().parents[2]
PROFILE_DIR = os.environ.get("DSSP_PROFILE_DIR") or str(_PYTHON_ROOT / ".pw-profile")

# Headed by default so the operator can complete the manual login.
HEADLESS = _flag("DSSP_HEADLESS", "0")
LOGIN_TIMEOUT_MS = int(os.environ.get("DSSP_LOGIN_TIMEOUT_MS", "300000"))
NAV_TIMEOUT_MS = int(os.environ.get("DSSP_NAV_TIMEOUT_MS", "30000"))


def dump_dir() -> str | None:
    """Where to write raw portal evidence, when ``DSSP_DUMP_DIR`` is set.

    A live-run diagnostic and nothing else: unset is the normal case, and no
    part of the run depends on it. Read per use rather than at import so one run
    (or one test) can turn it on without re-importing the module.
    """
    return os.environ.get("DSSP_DUMP_DIR") or None
