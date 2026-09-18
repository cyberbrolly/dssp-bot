"""HTML parsing for the DSSP portal.

Pure functions over BeautifulSoup documents — no network, no browser. Ported
from DSSPPortalAdapter.ts. This task covers the read side (trainees, form
options, session detection, normalisation); the submit side (payload building
and outcome classification) is added in a later task.
"""

from __future__ import annotations

import json
import re
from datetime import date
from typing import Any, Optional
from urllib.parse import urlparse

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


# -- form filling / payload -------------------------------------------------
def get_training_form_fields(soup: BeautifulSoup) -> dict[str, Tag]:
    """Locate the Log New Training form and its three required controls."""
    training_date = find_training_date_input(soup)
    if training_date is None:
        raise e.element_not_found("Training Date input")

    instructor = find_instructor_select(soup)
    if instructor is None:
        raise e.element_not_found("Instructor select")

    training_type = find_training_type_select(soup)
    if training_type is None:
        raise e.element_not_found("Training Type select")

    form: Optional[Tag] = None
    for control in (training_date, instructor, training_type):
        form = control.find_parent("form")
        if form is not None:
            break

    if form is None:
        raise e.element_not_found("Log New Training form")

    return {
        "form": form,
        "training_date": training_date,
        "instructor": instructor,
        "training_type": training_type,
    }


def select_form_option(
    select: Tag, selected_value: str, selected_label: Optional[str] = None
) -> dict[str, str]:
    """Choose an option by value or label, mirroring selectFormOption.

    With a label, both value and label must match; without, either may match."""
    options = [
        option
        for option in select.select("option")
        if not option.has_attr("disabled") and _option_value(option)
    ]

    def matches(option: Tag) -> bool:
        if selected_label is not None:
            return (
                _option_value(option) == selected_value
                and _option_label(option) == selected_label
            )
        return (
            _option_value(option) == selected_value
            or _option_label(option) == selected_value
        )

    matched = next((option for option in options if matches(option)), None)
    if matched is None:
        name = select.get("name") or select.get("id") or "select"
        raise e.validation(f"The selected value is not present in {name}.")

    return {"value": _option_value(matched), "label": _option_label(matched)}


def build_form_payload(
    soup: BeautifulSoup, session: dict[str, str]
) -> tuple[Tag, list[tuple[str, str]]]:
    """Virtually fill the training form and build its POST payload.

    Port of fillTrainingFormFields + validateTrainingForm +
    buildTrainingFormPayload: every named control (hidden fields and the
    submitter included) is collected in DOM order, then the three session
    values are applied."""
    fields = get_training_form_fields(soup)
    form = fields["form"]

    method = (form.get("method") or "get").strip().upper()
    if method != "POST":
        raise e.portal_structure(
            f"The Log New Training form uses unsupported method {method}."
        )

    payload: list[tuple[str, str]] = []

    for element in form.find_all(["input", "select", "textarea"]):
        if element.has_attr("disabled"):
            continue
        name = element.get("name")
        if not name:
            continue

        if element.name == "input":
            type_ = (element.get("type") or "text").lower()
            if type_ == "file":
                raise e.portal_structure(
                    "The training form contains an unsupported file field."
                )
            if type_ in ("checkbox", "radio"):
                if element.has_attr("checked"):
                    payload.append((name, element.get("value") or "on"))
            elif type_ in ("submit", "button", "reset", "image"):
                continue
            else:
                payload.append((name, element.get("value") or ""))
        elif element.name == "select":
            selected = [
                option for option in element.select("option") if option.has_attr("selected")
            ]
            if not selected:
                selected = [
                    option
                    for option in element.select("option")
                    if not option.has_attr("disabled")
                ][:1]
            for option in selected:
                payload.append((name, _option_value(option)))
        else:  # textarea
            payload.append((name, element.get_text()))

    submitter = form.select_one(c.SUBMIT_SELECTOR)
    if submitter is not None and submitter.get("name"):
        payload.append((submitter["name"], submitter.get("value") or ""))

    # Apply the session values (fillTrainingFormFields).
    date_name = fields["training_date"].get("name")
    instructor_name = fields["instructor"].get("name")
    type_name = fields["training_type"].get("name")
    if not date_name or not instructor_name or not type_name:
        raise e.portal_structure(
            "A required training control has no form field name."
        )

    overrides = {
        date_name: format_training_date(session["training_date"]),
        instructor_name: select_form_option(
            fields["instructor"], session["instructor"], session.get("instructor_label")
        )["value"],
        type_name: select_form_option(
            fields["training_type"],
            session["training_type"],
            session.get("training_type_label"),
        )["value"],
    }

    overridden = set(overrides)
    payload = [(k, v) for k, v in payload if k not in overridden]
    payload.extend(overrides.items())

    return form, payload


# -- submission outcome -----------------------------------------------------
_DUPLICATE_RE = re.compile(
    r"duplicate|already\s+(?:logged|recorded|exists?)", re.IGNORECASE
)
_SUCCESS_RE = re.compile(r"success|successfully|saved|recorded|created", re.IGNORECASE)


def validation_message(soup: BeautifulSoup) -> Optional[str]:
    messages = [
        " ".join(el.get_text().split())
        for el in soup.select(c.VALIDATION_MESSAGE_SELECTOR)
    ]
    messages = [message for message in messages if message]
    return " ".join(messages) if messages else None


def message_from_json(body: str) -> Optional[dict[str, Any]]:
    try:
        value = json.loads(body)
    except (ValueError, TypeError):
        return None
    if not isinstance(value, dict):
        return None

    result: dict[str, Any] = {}
    success = value.get("success", value.get("Success"))
    message = value.get("message")
    if message is None:
        message = value.get("Message")
    if message is None:
        message = value.get("error")
    if message is None:
        message = value.get("Error")

    if isinstance(success, bool):
        result["success"] = success
    if isinstance(message, str):
        result["message"] = message
    return result


def submission_outcome(
    body: str, final_url: str, status: int, redirected: bool
) -> dict[str, str]:
    """Classify a submission response. Port of submissionOutcome.

    Returns {"outcome": "confirmed"|"duplicate"|"rejected"|"indeterminate"}
    plus reference/message where applicable. Raises SESSION_EXPIRED when the
    response is the login page. Classification only — never resubmits."""
    soup = parse_html(body)

    if is_login_page(soup, final_url):
        raise e.session_expired()

    json_message = message_from_json(body)
    visible_text = (
        " ".join(soup.body.get_text().split()) if soup.body else body.strip()
    )
    message = (
        json_message.get("message", visible_text) if json_message else visible_text
    )

    if _DUPLICATE_RE.search(message):
        return {"outcome": "duplicate", "message": message}

    validation = validation_message(soup)

    if (
        (json_message is not None and json_message.get("success") is False)
        or validation is not None
        or status >= 400
    ):
        detail = (
            (json_message.get("message") if json_message else None)
            or validation
            or f"DSSP returned HTTP {status}."
        )
        return {"outcome": "rejected", "message": detail}

    final_path = urlparse(final_url).path
    if (
        (json_message is not None and json_message.get("success") is True)
        or _SUCCESS_RE.search(message)
        or status == 204
        or (200 <= status < 300 and not body.strip())
        or (redirected and "/TrainingLog" not in final_path)
    ):
        return {"outcome": "confirmed", "reference": final_url}

    return {
        "outcome": "indeterminate",
        "message": f"DSSP returned HTTP {status} without a recognizable result.",
    }
