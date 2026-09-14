"""HTML parsing for the DSSP portal.

Pure functions over BeautifulSoup documents — no network, no browser. Ported
from DSSPPortalAdapter.ts. This task covers the read side (trainees, form
options, session detection, normalisation); the submit side (payload building
and outcome classification) is added in a later task.
"""

from __future__ import annotations

import re
from datetime import date
from typing import Any, Optional

from bs4 import BeautifulSoup
from bs4.element import Tag

from . import constants as c
from . import errors as e


def parse_html(html: str) -> BeautifulSoup:
    return BeautifulSoup(html or "", "lxml")


# -- normalisation ----------------------------------------------------------
def normalize_name(value: str) -> str:
    """Collapse whitespace and upper-case, so 'John  Doe' == 'JOHN DOE'."""
    return " ".join((value or "").split()).upper()


_ISO_DATE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")
_YEAR_FIRST = re.compile(r"^(\d{4})/([0-1]?\d)/([0-3]?\d)$")
_DAY_FIRST = re.compile(r"^(\d{1,2})[/-](\d{1,2})[/-](\d{4})$")


def _checked_date(year: int, month: int, day: int) -> str:
    try:
        return date(year, month, day).strftime("%Y-%m-%d")
    except ValueError as exc:
        raise e.validation("Training date must be a real calendar date.") from exc


def format_training_date(value: str) -> str:
    """Normalise a date to YYYY-MM-DD. Accepts ISO, YYYY/M/D, DD/MM/YYYY."""
    text = (value or "").strip()

    match = _ISO_DATE.match(text)
    if match:
        return _checked_date(int(match[1]), int(match[2]), int(match[3]))

    match = _YEAR_FIRST.match(text)
    if match:
        return _checked_date(int(match[1]), int(match[2]), int(match[3]))

    match = _DAY_FIRST.match(text)
    if match:
        return _checked_date(int(match[3]), int(match[2]), int(match[1]))

    raise e.validation("Training date must use YYYY-MM-DD (or DD/MM/YYYY) format.")


# -- trainees ---------------------------------------------------------------
def _cell_text(cells: list[Tag], index: int) -> str:
    return cells[index].get_text().strip() if 0 <= index < len(cells) else ""


def trainee_id_from_href(href: str) -> Optional[str]:
    match = c.TRAINEE_ID_RE.search(href or "")
    return match.group(1) if match else None


def get_trainees(soup: BeautifulSoup) -> list[dict[str, Any]]:
    trainees: list[dict[str, Any]] = []

    for row in soup.select(c.TRAINEE_ROW_SELECTOR):
        cells = row.select("td")
        log_link = row.select_one(c.TRAINING_LOG_LINK_SELECTOR)
        id_link = row.select_one(c.TRAINEE_ID_LINK_SELECTOR)
        href = (
            (log_link.get("href") if log_link else None)
            or (id_link.get("href") if id_link else None)
            or ""
        )
        trainee_id = trainee_id_from_href(href)
        if not trainee_id:
            continue

        trainees.append(
            {
                "id": trainee_id,
                "trainee_id": trainee_id,
                "sn": _cell_text(cells, 0),
                "application_date": _cell_text(cells, 1),
                "name": _cell_text(cells, 2),
                "dob": _cell_text(cells, 3),
                "course": _cell_text(cells, 4),
                "phone": _cell_text(cells, 5),
                "email": _cell_text(cells, 6),
                "training_sessions": _cell_text(cells, 7),
                "assessment_score": _cell_text(cells, 8),
                "last_modified": _cell_text(cells, 9),
                "modified_by": _cell_text(cells, 10),
                "profile_url": (
                    log_link.get("href")
                    if log_link
                    else c.training_form_url(trainee_id)
                ),
            }
        )

    return trainees


# -- form options -----------------------------------------------------------
def _option_label(option: Tag) -> str:
    text = option.get_text()
    return (text if text.strip() else option.get("label", "")).strip()


def _option_value(option: Tag) -> str:
    # A DOM <option> without a value attribute reports its text as the value.
    raw = option.get("value")
    return (raw if raw is not None else option.get_text()).strip()


def get_select_options(select: Tag) -> list[dict[str, str]]:
    options: list[dict[str, str]] = []
    for option in select.select("option"):
        if option.has_attr("disabled"):
            continue
        value = _option_value(option)
        label = _option_label(option)
        if not value or not label:
            continue
        options.append({"value": value, "label": label})
    return options


def _query_first(soup: BeautifulSoup, selectors: tuple[str, ...]) -> Optional[Tag]:
    for selector in selectors:
        element = soup.select_one(selector)
        if element is not None:
            return element
    return None


def _identity(element: Tag) -> str:
    ident = f"{element.get('id', '')} {element.get('name', '')}"
    return re.sub(r"[^a-z0-9]+", " ", ident, flags=re.IGNORECASE).lower()


def _token_control(
    soup: BeautifulSoup, tag_name: str, tokens: tuple[str, ...]
) -> Optional[Tag]:
    for element in soup.find_all(tag_name):
        identity = _identity(element)
        if all(token in identity for token in tokens):
            return element
    return None


def _label_text(label: Tag) -> str:
    return " ".join(label.get_text().split())


def _labelled_control(
    soup: BeautifulSoup, tag_name: str, pattern: re.Pattern[str]
) -> Optional[Tag]:
    for label in soup.find_all("label"):
        if not pattern.search(_label_text(label)):
            continue

        control: Optional[Tag] = None
        for_id = label.get("for")
        if for_id:
            control = soup.find(id=for_id)
        if control is None:
            control = label.find(tag_name)
        if control is None and label.parent is not None:
            control = label.parent.find(tag_name)
        if control is not None and control.name == tag_name:
            return control
    return None


_INSTRUCTOR_RE = re.compile(r"instructor", re.IGNORECASE)
_TRAINING_TYPE_RE = re.compile(r"training\s*type", re.IGNORECASE)
_TRAINING_DATE_RE = re.compile(r"training\s*date|date.*yyyy", re.IGNORECASE)


def find_instructor_select(soup: BeautifulSoup) -> Optional[Tag]:
    return (
        _query_first(soup, c.INSTRUCTOR_SELECTORS)
        or _labelled_control(soup, "select", _INSTRUCTOR_RE)
        or _token_control(soup, "select", ("instructor",))
    )


def find_training_type_select(soup: BeautifulSoup) -> Optional[Tag]:
    return (
        _query_first(soup, c.TRAINING_TYPE_SELECTORS)
        or _labelled_control(soup, "select", _TRAINING_TYPE_RE)
        or _token_control(soup, "select", ("training", "type"))
    )


def find_training_date_input(soup: BeautifulSoup) -> Optional[Tag]:
    return (
        _query_first(soup, c.TRAINING_DATE_SELECTORS)
        or _labelled_control(soup, "input", _TRAINING_DATE_RE)
        or _token_control(soup, "input", ("training", "date"))
    )


def get_form_options(soup: BeautifulSoup) -> dict[str, list[dict[str, str]]]:
    instructor = find_instructor_select(soup)
    if instructor is None:
        raise e.element_not_found("Instructor select")

    training_type = find_training_type_select(soup)
    if training_type is None:
        raise e.element_not_found("Training Type select")

    instructors = get_select_options(instructor)
    training_types = get_select_options(training_type)

    if not instructors:
        raise e.missing_data("Instructor options")
    if not training_types:
        raise e.missing_data("Training Type options")

    return {"instructors": instructors, "training_types": training_types}


# -- session detection ------------------------------------------------------
def is_login_page(soup: BeautifulSoup, href: str) -> bool:
    from urllib.parse import urlparse

    try:
        path = urlparse(href).path or "/"
        if path.startswith("/Account/Login"):
            return True
    except Exception:
        return True

    return soup.select_one(c.LOGIN_FORM_SELECTOR) is not None


def has_authenticated_marker(soup: BeautifulSoup) -> bool:
    if soup.select_one(c.TRAINEE_TABLE_SELECTOR):
        return True
    if soup.select_one(c.LOGOUT_SELECTOR):
        return True
    return any(
        re.search(r"enrolled\s+trainees", el.get_text(), re.IGNORECASE)
        for el in soup.select("h1, h2, .page-title")
    )
