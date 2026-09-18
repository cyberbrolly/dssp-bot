"""Parsing and classification regression tests (no browser, no network)."""

import pytest

from app.portal import errors as e
from app.portal import parsing as parse

TRAINEES_HTML = """
<table class="table-checkable"><tbody>
  <tr><td>1</td><td>2024-01-01</td><td>John   Doe</td><td>x</td><td>Class C</td>
      <td>0803</td><td>a@b.com</td><td>2</td><td>80</td><td>d</td><td>admin</td>
      <td><a href="/Trainee/TrainingLog/TraineeId=123">Log</a></td></tr>
  <tr><td>2</td><td>2024-01-02</td><td>Jane Roe</td><td>x</td><td>Class B</td>
      <td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee?TraineeId=456">Open</a></td></tr>
  <tr><td>3</td><td>2024-01-03</td><td>No Link</td><td>x</td><td>Class B</td>
      <td></td><td></td><td>0</td><td></td><td></td><td></td><td>none</td></tr>
</tbody></table>
"""

FORM_HTML = """
<form action="/post/123" method="post">
  <input type="hidden" name="__RequestVerificationToken" value="TOK"/>
  <input type="text" id="TrainingDate" name="TrainingDate" value="2020-01-01"/>
  <select id="Instructor" name="Instructor">
    <option value="">--</option><option value="10">Alice</option><option value="11" disabled>Bob</option>
  </select>
  <select id="TrainingType" name="TrainingType">
    <option value="">--</option><option value="1">Practical</option><option value="2" selected>Theory</option>
  </select>
  <input type="checkbox" name="Agree" checked/>
  <button type="submit" name="submitBtn" value="save">Save</button>
</form>
"""

LOGIN_HTML = '<form id="loginForm" action="/Account/Login"><input name="Password"/></form>'

SESSION = {
    "training_date": "14/09/2026",
    "instructor": "Alice",
    "training_type": "2",
}


# -- trainees ---------------------------------------------------------------
def test_get_trainees_extracts_rows_and_ids():
    trainees = parse.get_trainees(parse.parse_html(TRAINEES_HTML))
    assert [t["id"] for t in trainees] == ["123", "456"]
    assert trainees[0]["name"] == "John   Doe"
    assert trainees[0]["course"] == "Class C"
    assert trainees[0]["profile_url"] == "/Trainee/TrainingLog/TraineeId=123"
    assert trainees[1]["profile_url"] == "/Trainee?TraineeId=456"


def test_get_trainees_skips_rows_without_trainee_id():
    trainees = parse.get_trainees(parse.parse_html(TRAINEES_HTML))
    assert all(t["name"] != "No Link" for t in trainees)


# -- form options -----------------------------------------------------------
def test_get_form_options():
    options = parse.get_form_options(parse.parse_html(FORM_HTML))
    assert options["instructors"] == [{"value": "10", "label": "Alice"}]
    assert options["training_types"] == [
        {"value": "1", "label": "Practical"},
        {"value": "2", "label": "Theory"},
    ]


def test_get_form_options_missing_control():
    with pytest.raises(e.PortalError) as info:
        parse.get_form_options(parse.parse_html("<html></html>"))
    assert info.value.error_code == "ELEMENT_NOT_FOUND"


# -- session detection ------------------------------------------------------
def test_login_and_authenticated_detection():
    login = parse.parse_html(LOGIN_HTML)
    assert parse.is_login_page(login, "https://dssp.frsc.gov.ng/Account/Login")
    assert parse.is_login_page(login, "https://dssp.frsc.gov.ng/Trainee")
    assert not parse.has_authenticated_marker(login)

    authed = parse.parse_html(TRAINEES_HTML)
    assert parse.has_authenticated_marker(authed)
    assert not parse.is_login_page(authed, "https://dssp.frsc.gov.ng/Trainee")


# -- normalisation ----------------------------------------------------------
def test_normalize_name():
    assert parse.normalize_name("John   Doe") == "JOHN DOE"
    assert parse.normalize_name(" john doe ") == "JOHN DOE"


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("2026-09-14", "2026-09-14"),
        ("2026/9/3", "2026-09-03"),
        ("13/09/2026", "2026-09-13"),
        ("13-09-2026", "2026-09-13"),
    ],
)
def test_format_training_date_valid(raw, expected):
    assert parse.format_training_date(raw) == expected


@pytest.mark.parametrize("raw", ["2026-13-40", "not a date", ""])
def test_format_training_date_invalid(raw):
    with pytest.raises(e.PortalError) as info:
        parse.format_training_date(raw)
    assert info.value.error_code == "VALIDATION_FAILED"


# -- payload building -------------------------------------------------------
def test_build_form_payload_applies_session_values():
    form, payload = parse.build_form_payload(parse.parse_html(FORM_HTML), SESSION)
    assert form.get("method") == "post"

    data = dict(payload)  # duplicates collapse last-wins for the assertion only
    assert data["__RequestVerificationToken"] == "TOK"
    assert data["TrainingDate"] == "2026-09-14"
    assert data["Instructor"] == "10"  # matched by label "Alice"
    assert data["TrainingType"] == "2"
    assert data["Agree"] == "on"
    assert data["submitBtn"] == "save"


def test_build_form_payload_rejects_non_post_form():
    html = FORM_HTML.replace('method="post"', 'method="get"')
    with pytest.raises(e.PortalError) as info:
        parse.build_form_payload(parse.parse_html(html), SESSION)
    assert info.value.error_code == "PORTAL_STRUCTURE_CHANGED"


def test_select_form_option_rejects_unknown_value():
    soup = parse.parse_html(FORM_HTML)
    select = parse.find_instructor_select(soup)
    with pytest.raises(e.PortalError) as info:
        parse.select_form_option(select, "Charlie")
    assert info.value.error_code == "VALIDATION_FAILED"


# -- submission outcome -----------------------------------------------------
def test_outcome_confirmed_via_redirect_away_from_form():
    outcome = parse.submission_outcome(
        TRAINEES_HTML, "https://x/Trainee?saved=1", 200, True
    )
    assert outcome["outcome"] == "confirmed"


def test_outcome_duplicate():
    outcome = parse.submission_outcome(
        "<html><body>Record already logged</body></html>", "https://x/post", 200, False
    )
    assert outcome["outcome"] == "duplicate"


def test_outcome_rejected_validation():
    body = (
        "<html><body><div class='validation-summary-errors'>Date is invalid"
        "</div></body></html>"
    )
    outcome = parse.submission_outcome(body, "https://x/post", 200, False)
    assert outcome["outcome"] == "rejected"
    assert outcome["message"] == "Date is invalid"


def test_outcome_rejected_http_error():
    outcome = parse.submission_outcome("", "https://x/post", 500, False)
    assert outcome["outcome"] == "rejected"


def test_outcome_rejected_json_success_false():
    outcome = parse.submission_outcome(
        '{"success": false, "message": "nope"}', "https://x/post", 200, False
    )
    assert outcome["outcome"] == "rejected"
    assert outcome["message"] == "nope"


def test_outcome_confirmed_json_success_true():
    outcome = parse.submission_outcome(
        '{"success": true}', "https://x/post", 200, False
    )
    assert outcome["outcome"] == "confirmed"


def test_outcome_confirmed_empty_204():
    outcome = parse.submission_outcome("", "https://x/post", 204, False)
    assert outcome["outcome"] == "confirmed"


def test_outcome_indeterminate_when_unreadable():
    outcome = parse.submission_outcome("banana", "https://x/post", 200, False)
    assert outcome["outcome"] == "indeterminate"


def test_outcome_login_page_raises_session_expired():
    with pytest.raises(e.PortalError) as info:
        parse.submission_outcome(LOGIN_HTML, "https://x/Account/Login", 200, False)
    assert info.value.error_code == "SESSION_EXPIRED"
