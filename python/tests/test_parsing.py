"""Parsing and classification regression tests (no browser, no network)."""

import json

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
<form id="logoutForm" action="/Account/LogOff?area=" method="post">
  <input type="hidden" name="__RequestVerificationToken" value="SYNTHETIC_LOGOUT_TOKEN"/>
</form>
<form name="frmtraininglog" id="frmtraininglog">
  <input type="hidden" id="TraineeId" name="TraineeId" value="123"/>
  <input type="text" id="TrainingDate" name="TrainingDate" value=""/>
  <select id="InstructorId" name="InstructorId">
    <option value="">--</option><option value="10">Alice</option><option value="11" disabled>Bob</option>
  </select>
  <select id="TrainingOptionId" name="TrainingOptionId" onchange="onTrainingOptionChanged()">
    <option value="">--</option><option value="1">Practical</option><option value="2">Theory</option>
  </select>
  <div id="final-assessment-tab" style="display:none">
    <input type="hidden" name="FinalAssessments[0].TraineeId" value="123"/>
    <input type="hidden" name="FinalAssessments[0].FinalAssessmentId" value="1"/>
    <input type="number" name="FinalAssessments[0].MarkObtained" value="8"/>
  </div>
</form>
<button type="button" onclick="onTrainingLogSubmit()">Log Training</button>
"""

LOGIN_HTML = '<form id="loginForm" action="/Account/Login"><input name="Password"/></form>'

SESSION = {
    "training_date": "14/09/2026",
    "instructor": "Alice",
    "training_type": "2",
}


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


def test_get_form_options():
    options = parse.get_form_options(parse.parse_html(FORM_HTML))
    assert options["instructors"] == [{"value": "10", "label": "Alice"}]
    assert options["training_types"] == [
        {"value": "1", "label": "Practical"},
        {"value": "2", "label": "Theory"},
    ]


def ajax_form_html():
    return FORM_HTML


def test_ajax_form_options_without_method_action_or_submitter():
    soup = parse.parse_html(ajax_form_html())
    fields = parse.get_training_form_fields(soup)
    assert fields["training_type"]["id"] == "TrainingOptionId"
    assert fields["instructor"]["id"] == "InstructorId"
    assert not fields["form"].has_attr("action")
    assert not fields["form"].has_attr("method")
    assert parse.get_form_options(soup)["training_types"] == [
        {"value": "1", "label": "Practical"},
        {"value": "2", "label": "Theory"},
    ]


def test_controls_in_another_form_do_not_override_training_form():
    decoy = FORM_HTML.replace('id="frmtraininglog"', 'id="other"').replace("Alice", "Wrong")
    soup = parse.parse_html(decoy + ajax_form_html())
    fields = parse.get_training_form_fields(soup)
    assert all(control.find_parent("form") is fields["form"] for key, control in fields.items() if key != "form")
    assert parse.get_form_options(soup)["instructors"] == [{"value": "10", "label": "Alice"}]


@pytest.mark.parametrize("control_id", ["TrainingDate", "InstructorId", "TrainingOptionId"])
def test_missing_control_cannot_be_taken_from_outside_training_form(control_id):
    soup = parse.parse_html(ajax_form_html())
    control = soup.find(id=control_id).extract()
    soup.body.append(control)
    with pytest.raises(e.PortalError) as info:
        parse.get_form_options(soup)
    assert info.value.error_code == "ELEMENT_NOT_FOUND"


@pytest.mark.parametrize("control_id", ["TrainingDate", "InstructorId", "TrainingOptionId"])
def test_controls_explicitly_owned_by_another_form_are_rejected(control_id):
    soup = parse.parse_html(ajax_form_html())
    soup.find(id=control_id)["form"] = "other"
    with pytest.raises(e.PortalError) as info:
        parse.get_training_form_fields(soup)
    assert info.value.error_code == "PORTAL_STRUCTURE_CHANGED"


@pytest.mark.parametrize("control_id", ["TrainingDate", "InstructorId", "TrainingOptionId"])
def test_disabled_required_controls_are_rejected(control_id):
    soup = parse.parse_html(ajax_form_html())
    soup.find(id=control_id)["disabled"] = ""
    with pytest.raises(e.PortalError) as info:
        parse.get_training_form_fields(soup)
    assert info.value.error_code == "PORTAL_STRUCTURE_CHANGED"


def test_multiple_training_forms_are_rejected():
    with pytest.raises(e.PortalError) as info:
        parse.get_training_form_fields(parse.parse_html(ajax_form_html() * 2))
    assert info.value.error_code == "PORTAL_STRUCTURE_CHANGED"


def test_get_form_options_missing_control():
    with pytest.raises(e.PortalError) as info:
        parse.get_form_options(parse.parse_html("<html></html>"))
    assert info.value.error_code == "ELEMENT_NOT_FOUND"


def test_login_and_authenticated_detection():
    login = parse.parse_html(LOGIN_HTML)
    assert parse.is_login_page(login, "https://dssp.frsc.gov.ng/Account/Login")
    assert parse.is_login_page(login, "https://dssp.frsc.gov.ng/Trainee")
    assert not parse.has_authenticated_marker(login)

    authed = parse.parse_html(TRAINEES_HTML)
    assert parse.has_authenticated_marker(authed)
    assert not parse.is_login_page(authed, "https://dssp.frsc.gov.ng/Trainee")


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


def test_build_form_payload_applies_session_values():
    payload = parse.build_form_payload(parse.parse_html(FORM_HTML), SESSION, "123")
    assert payload == [
        ("LogDetails[TraineeId]", "123"),
        ("LogDetails[TrainingDate]", "2026-09-14"),
        ("LogDetails[InstructorId]", "10"),
        ("LogDetails[TrainingOptionId]", "2"),
    ]


@pytest.mark.parametrize("trainee_id", ["123", "456"])
def test_payload_uses_current_record_not_a_captured_id(trainee_id):
    html = FORM_HTML.replace('value="123"', f'value="{trainee_id}"')
    payload = dict(parse.build_form_payload(parse.parse_html(html), SESSION, trainee_id))
    assert payload["LogDetails[TraineeId]"] == trainee_id
    assert len(payload) == 4


@pytest.mark.parametrize("value", ["", "456", "abc", "123&other=456"])
def test_payload_rejects_invalid_or_mismatched_form_id(value):
    soup = parse.parse_html(FORM_HTML)
    soup.find(id="TraineeId")["value"] = value
    with pytest.raises(e.PortalError) as info:
        parse.build_form_payload(soup, SESSION, "123")
    assert info.value.error_code == "PORTAL_STRUCTURE_CHANGED"


@pytest.mark.parametrize("mutation", ["missing", "outside", "duplicate", "wrong_owner", "wrong_name", "wrong_id", "visible", "disabled"])
def test_payload_rejects_invalid_identity_control(mutation):
    soup = parse.parse_html(FORM_HTML)
    control = soup.find(id="TraineeId")
    if mutation == "missing":
        control.decompose()
    elif mutation == "outside":
        soup.body.append(control.extract())
    elif mutation == "duplicate":
        duplicate = soup.new_tag("input", id="TraineeId", type="hidden", value="123")
        duplicate["name"] = "TraineeId"
        control.insert_after(duplicate)
    else:
        attr, value = {
            "wrong_owner": ("form", "logoutForm"),
            "wrong_name": ("name", "OtherId"),
            "wrong_id": ("id", "OtherId"),
            "visible": ("type", "text"),
            "disabled": ("disabled", ""),
        }[mutation]
        control[attr] = value
    with pytest.raises(e.PortalError) as info:
        parse.build_form_payload(soup, SESSION, "123")
    assert info.value.error_code == "PORTAL_STRUCTURE_CHANGED"


@pytest.mark.parametrize("selected", ["4", "Final Assessment"])
def test_final_assessment_option_is_blocked(selected):
    soup = parse.parse_html(FORM_HTML)
    option = soup.new_tag("option", value="4")
    option.string = "Final Assessment"
    soup.find(id="TrainingOptionId").append(option)
    with pytest.raises(e.PortalError) as info:
        parse.build_form_payload(soup, {**SESSION, "training_type": selected}, "123")
    assert info.value.error_code == "VALIDATION_FAILED"
    assert "Final-assessment" in info.value.message


@pytest.mark.parametrize("key", ["final_assessments", "FinalAssessments"])
def test_explicit_assessment_payload_is_not_silently_ignored(key):
    with pytest.raises(e.PortalError) as info:
        parse.build_form_payload(
            parse.parse_html(FORM_HTML), {**SESSION, key: [{"MarkObtained": 8}]}, "123"
        )
    assert info.value.error_code == "VALIDATION_FAILED"


def test_select_form_option_rejects_unknown_value():
    soup = parse.parse_html(FORM_HTML)
    select = parse.find_instructor_select(soup)
    with pytest.raises(e.PortalError) as info:
        parse.select_form_option(select, "Charlie")
    assert info.value.error_code == "VALIDATION_FAILED"


def test_outcome_indeterminate_via_redirect_without_confirmation():
    outcome = parse.submission_outcome(
        TRAINEES_HTML, "https://x/Trainee?saved=1", 200, True
    )
    assert outcome["outcome"] == "indeterminate"


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


@pytest.mark.parametrize("body", ['{"success": true}', '{"Success": true}', '{"IsSuccessful": true}'])
def test_outcome_confirmed_json_success_true(body):
    outcome = parse.submission_outcome(body, "https://x/post", 200, False)
    assert outcome["outcome"] == "confirmed"


@pytest.mark.parametrize("status", [200, 204])
def test_outcome_indeterminate_empty_response(status):
    outcome = parse.submission_outcome("", "https://x/post", status, False)
    assert outcome["outcome"] == "indeterminate"


@pytest.mark.parametrize(
    "message",
    [
        "Training was not saved.",
        "Submission unsuccessful.",
        "No record was created.",
        "Training saved successfully.",
    ],
)
def test_outcome_indeterminate_without_verified_confirmation(message):
    outcome = parse.submission_outcome(
        f"<html><body>{message}</body></html>", "https://x/post", 200, False
    )
    assert outcome["outcome"] == "indeterminate"


@pytest.mark.parametrize(
    "body",
    ['{"success": "true"}', '{"success": 1}', '{"message": "saved"}'],
)
def test_outcome_indeterminate_without_boolean_json_success(body):
    outcome = parse.submission_outcome(body, "https://x/post", 200, False)
    assert outcome["outcome"] == "indeterminate"


def test_outcome_indeterminate_when_unreadable():
    outcome = parse.submission_outcome("banana", "https://x/post", 200, False)
    assert outcome["outcome"] == "indeterminate"


@pytest.mark.parametrize("key", ["IsSuccessful", "success", "Success"])
@pytest.mark.parametrize("status", [200, 201, 299])
def test_explicit_json_success_requires_2xx(key, status):
    outcome = parse.submission_outcome(json.dumps({key: True}), "https://x/post", status, False)
    assert outcome["outcome"] == "confirmed"


@pytest.mark.parametrize("key", ["IsSuccessful", "success", "Success"])
@pytest.mark.parametrize("status", [100, 300, 302, 303, 307, 308, 399])
def test_success_flag_on_non_success_status_is_indeterminate(key, status):
    outcome = parse.submission_outcome(json.dumps({key: True}), "https://x/post", status, True)
    assert outcome["outcome"] == "indeterminate"


@pytest.mark.parametrize("value", ["true", "false", 1, 0, None, [], {}])
def test_is_successful_non_boolean_is_indeterminate(value):
    body = json.dumps({"IsSuccessful": value, "Message": "Training saved successfully."})
    assert parse.submission_outcome(body, "https://x/post", 200, False)["outcome"] == "indeterminate"


@pytest.mark.parametrize("message", ["Date is invalid", "Record already logged"])
def test_is_successful_false_rejected_with_message(message):
    body = json.dumps({"IsSuccessful": False, "Message": message})
    assert parse.submission_outcome(body, "https://x/post", 200, False) == {
        "outcome": "rejected", "message": message,
    }


@pytest.mark.parametrize("status", [400, 500])
def test_http_error_overrides_json_success(status):
    assert parse.submission_outcome('{"IsSuccessful": true}', "https://x/post", status, False)["outcome"] == "rejected"


@pytest.mark.parametrize("value", [False, "true", None])
def test_is_successful_takes_precedence_over_legacy_alias(value):
    parsed = parse.message_from_json(json.dumps({"IsSuccessful": value, "success": True}))
    assert parsed.get("success") is not True


def test_outcome_login_page_raises_session_expired():
    with pytest.raises(e.PortalError) as info:
        parse.submission_outcome(LOGIN_HTML, "https://x/Account/Login", 200, False)
    assert info.value.error_code == "SESSION_EXPIRED"
