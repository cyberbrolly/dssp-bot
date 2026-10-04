"""Submission tests against a local DSSP-shaped fixture portal.

These exercise the real Playwright stack (headless, throwaway profile) but never
touch the real portal: the client's constants are patched to the fixture server.
"""

import json
import logging
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlsplit

import pytest

from app.portal import constants as c
from app.portal import errors as e
from app.portal.client import PortalClient

TRAINEES_HTML = """
<table class="table-checkable"><tbody>
  <tr><td>1</td><td>a</td><td>John Doe</td><td>x</td><td>Class C</td><td></td><td></td><td>2</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog?TraineeId=123">Log</a></td></tr>
  <tr><td>2</td><td>a</td><td>Jane Roe</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog?TraineeId=456">Log</a></td></tr>
  <tr><td>3</td><td>a</td><td>Weird Case</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog?TraineeId=555">Log</a></td></tr>
  <tr><td>4</td><td>a</td><td>Session Case</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog?TraineeId=666">Log</a></td></tr>
  <tr><td>5</td><td>a</td><td>John Doe</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog?TraineeId=789">Log</a></td></tr>
  <tr><td>6</td><td>a</td><td>Redirect Case</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog?TraineeId=777">Log</a></td></tr>
</tbody></table>
"""

FORM_HTML = """
<form id="frmtraininglog">
  <input type="hidden" id="TraineeId" name="TraineeId" value="__TID__"/>
  <input type="text" id="TrainingDate" name="TrainingDate" value="2020-01-01"/>
  <select id="InstructorId" name="InstructorId">
    <option value="">--</option><option value="10">Alice</option><option value="11">Bob</option>
  </select>
  <select id="TrainingOptionId" name="TrainingOptionId">
    <option value="">--</option><option value="1">Practical</option><option value="2" selected>Theory</option><option value="3">Simulator</option>
  </select>
  <input type="hidden" name="FinalAssessments[0].TraineeId" value="987"/>
  <input type="hidden" name="FinalAssessments[0].Result" value="Pass"/>
  <input type="text" name="Unrelated" value="not-training-data"/>
  <input type="checkbox" name="Agree" checked/>
</form>
<button type="button" name="submitBtn" value="save" onclick="onTrainingLogSubmit()">Save</button>
<form id="logoutForm" action="/Account/LogOff?area=" method="post">
  <input type="hidden" name="__RequestVerificationToken" value="SYNTHETIC-LOGOUT-TOKEN"/>
</form>
"""

LAST_POST = {}
SERVER_STATE = {}
REQUESTS = []


class _Handler(BaseHTTPRequestHandler):
    def _send(self, code, body, headers=None, content_type="text/html; charset=utf-8"):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", content_type)
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        REQUESTS.append(("GET", self.path))
        if self.path == "/redirect-target":
            self._send(200, '{"IsSuccessful": true}', content_type="application/json")
        elif self.path.startswith("/Trainee/TrainingLog"):
            redirect = SERVER_STATE.get("form_redirect")
            if redirect and self.path != redirect:
                self._send(302, "", {"Location": redirect})
                return
            tid = parse_qs(urlsplit(self.path).query).get("TraineeId", [""])[0]
            form = SERVER_STATE.get("form", FORM_HTML)
            self._send(200, form.replace("__TID__", tid))
        elif self.path.startswith("/Trainee"):
            self._send(200, TRAINEES_HTML)
        else:
            self._send(200, "<html></html>")

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        post_count = LAST_POST.get("count", 0) + 1
        LAST_POST.clear()
        LAST_POST["count"] = post_count
        LAST_POST["path"] = self.path
        LAST_POST["body"] = self.rfile.read(length).decode()
        LAST_POST["content_type"] = self.headers.get("Content-Type")
        LAST_POST["xrw"] = self.headers.get("X-Requested-With")
        LAST_POST["accept"] = self.headers.get("Accept")
        REQUESTS.append(("POST", self.path))

        if self.path == "/redirect-target":
            self._send(200, '{"IsSuccessful": true}', content_type="application/json")
            return
        assert self.path == "/Trainee/LogTraining"
        tid = parse_qs(LAST_POST["body"])["LogDetails[TraineeId]"][0]
        if "post_response" in SERVER_STATE:
            code, body, headers = SERVER_STATE["post_response"]
            self._send(code, body, headers, content_type="application/json")
        elif tid == "123":
            self._send(200, '{"IsSuccessful": true}', content_type="application/json")
        elif tid == "777":
            self._send(302, "", {"Location": "/redirect-target"})
        elif tid == "456":
            self._send(200, "<html><body>Record already logged</body></html>")
        elif tid == "789":
            self._send(
                200,
                '{"IsSuccessful": false, "Message": "Date is invalid"}',
                content_type="application/json",
            )
        elif tid == "555":
            self._send(200, "<html><body>banana</body></html>")
        elif tid == "666":
            self._send(200, '<form id="loginForm" action="/Account/Login"></form>')
        else:
            self._send(200, "<html></html>")

    def log_message(self, *args):
        pass


@pytest.fixture(scope="module")
def portal_server():
    httpd = HTTPServer(("127.0.0.1", 0), _Handler)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    yield httpd
    httpd.shutdown()
    thread.join()
    httpd.server_close()


@pytest.fixture()
def client(portal_server, monkeypatch):
    origin = f"http://127.0.0.1:{portal_server.server_address[1]}"
    monkeypatch.setattr(c, "DSSP_ORIGIN", origin)
    monkeypatch.setattr(c, "TRAINEE_PAGE_URL", f"{origin}/Trainee")
    monkeypatch.setattr(
        c, "TRAINEE_LIST_URL", f"{origin}/Trainee?pgsize=10000&page=1&keywords="
    )
    LAST_POST.clear()
    SERVER_STATE.clear()
    REQUESTS.clear()
    portal = PortalClient()
    yield portal
    portal.close()


def session(**overrides):
    values = {
        "training_date": "14/09/2026",
        "instructor": "Alice",
        "training_type": "2",
    }
    values.update(overrides)
    return values


def test_confirmed_payload_and_classification(client):
    result = client.submit_training({"id": "123"}, session())
    assert result["outcome"] == "confirmed"
    assert result["trainee"] == {"id": "123", "name": "John Doe"}
    assert result["attempts"] == 1
    assert result["reference"].endswith("/Trainee/LogTraining")

    body = LAST_POST["body"]
    assert body == (
        "LogDetails%5BTraineeId%5D=123"
        "&LogDetails%5BTrainingDate%5D=2026-09-14"
        "&LogDetails%5BInstructorId%5D=10"
        "&LogDetails%5BTrainingOptionId%5D=2"
    )
    for forbidden in (
        "FinalAssessments", "__RequestVerificationToken", "SYNTHETIC-LOGOUT-TOKEN",
        "Unrelated", "not-training-data", "Agree", "submitBtn",
    ):
        assert forbidden not in body
    assert LAST_POST["path"] == "/Trainee/LogTraining"
    assert LAST_POST["count"] == 1
    assert LAST_POST["content_type"] == "application/x-www-form-urlencoded; charset=UTF-8"
    assert LAST_POST["xrw"] == "XMLHttpRequest"
    assert LAST_POST["accept"] == "application/json"


def test_duplicate(client):
    result = client.submit_training({"id": "456"}, session(training_type="Theory"))
    assert result["outcome"] == "duplicate"
    assert "already logged" in result["message"]


def test_rejected(client):
    result = client.submit_training(
        {"id": "789"}, session(instructor="10", training_type="1")
    )
    assert result["outcome"] == "rejected"
    assert result["message"] == "Date is invalid"


def test_indeterminate(client):
    result = client.submit_training(
        {"id": "555"}, session(instructor="10", training_type="1")
    )
    assert result["outcome"] == "indeterminate"
    assert result["attempts"] == 1
    assert LAST_POST["count"] == 1


def test_redirect_without_confirmation_is_indeterminate(client):
    result = client.submit_training({"id": "777"}, session())
    assert result["outcome"] == "indeterminate"
    assert result["attempts"] == 1
    assert LAST_POST["count"] == 1
    assert not any(path == "/redirect-target" for _, path in REQUESTS)


def test_session_expired_on_post(client):
    with pytest.raises(e.PortalError) as info:
        client.submit_training(
            {"id": "666"}, session(instructor="10", training_type="1")
        )
    assert info.value.error_code == "SESSION_EXPIRED"


def test_unknown_id(client):
    with pytest.raises(e.PortalError) as info:
        client.submit_training({"id": "999"}, session())
    assert info.value.error_code == "TRAINEE_NOT_FOUND"


def test_ambiguous_name_never_guesses(client):
    with pytest.raises(e.PortalError) as info:
        client.submit_training({"name": "john  doe"}, session())
    assert info.value.error_code == "TRAINEE_NOT_FOUND"
    assert "Ambiguous" in info.value.message


def test_name_resolution_to_single_match(client):
    result = client.submit_training({"name": "jane  roe"}, session(instructor="10"))
    assert result["outcome"] == "duplicate"
    assert result["trainee"]["id"] == "456"


def test_bad_instructor_value_is_validation_error(client):
    with pytest.raises(e.PortalError) as info:
        client.submit_training({"id": "123"}, session(instructor="Charlie"))
    assert info.value.error_code == "VALIDATION_FAILED"


@pytest.mark.parametrize("status", [302, 307, 308])
def test_success_json_redirect_is_unconfirmed_and_never_followed(client, status):
    SERVER_STATE["post_response"] = (
        status, '{"IsSuccessful": true}', {"Location": "/redirect-target"},
    )

    result = client.submit_training({"id": "123"}, session())

    assert result["outcome"] == "indeterminate"
    assert result["attempts"] == 1
    assert LAST_POST["count"] == 1
    assert [(method, path) for method, path in REQUESTS if method == "POST"] == [
        ("POST", "/Trainee/LogTraining")
    ]
    assert not any(path == "/redirect-target" for _, path in REQUESTS)


@pytest.mark.parametrize("status", [302, 307, 308])
def test_login_redirect_is_session_expired_without_following(client, status):
    SERVER_STATE["post_response"] = (
        status, '{"IsSuccessful": true}', {"Location": "/Account/Login?ReturnUrl=%2FTrainee"},
    )

    with pytest.raises(e.PortalError) as info:
        client.submit_training({"id": "123"}, session())

    assert info.value.error_code == "SESSION_EXPIRED"
    assert LAST_POST["count"] == 1
    assert not any(path.startswith("/Account/Login") for _, path in REQUESTS)


def assert_validation_without_post(client, expected_code="VALIDATION_FAILED", **overrides):
    with pytest.raises(e.PortalError) as info:
        client.submit_training({"id": "123"}, session(**overrides))
    assert info.value.error_code == expected_code
    assert LAST_POST == {}
    assert not any(method == "POST" for method, _ in REQUESTS)


@pytest.mark.parametrize(
    "replacement",
    [
        "",
        '<input type="hidden" id="TraineeId" name="TraineeId" value="456"/>',
        '<input type="hidden" id="TraineeId" name="TraineeId" value="not-numeric"/>',
        '<input type="hidden" id="TraineeId" name="TraineeId" value=""/>',
        '<input type="text" id="TraineeId" name="TraineeId" value="123"/>',
        '<input type="hidden" id="TraineeId" value="123"/>',
        '<input type="hidden" name="TraineeId" value="123"/>',
        '<input type="hidden" id="TraineeId" name="TraineeId" value="123" form="logoutForm"/>',
    ],
    ids=["missing", "mismatch", "nonnumeric", "empty", "not-hidden", "missing-name", "missing-id", "misowned"],
)
def test_invalid_hidden_trainee_id_never_posts(client, replacement):
    SERVER_STATE["form"] = FORM_HTML.replace(
        '<input type="hidden" id="TraineeId" name="TraineeId" value="__TID__"/>',
        replacement,
    )
    assert_validation_without_post(client, expected_code="PORTAL_STRUCTURE_CHANGED")


@pytest.mark.parametrize("control", ["TraineeId", "TrainingDate", "InstructorId", "TrainingOptionId"])
@pytest.mark.parametrize("placement", ["missing", "outside", "other-form", "explicit-other-owner"])
def test_required_controls_must_belong_to_training_form(client, control, placement):
    # Keep the same valid control elsewhere so a document-wide lookup cannot
    # accidentally satisfy the training form's requirements.
    from bs4 import BeautifulSoup

    soup = BeautifulSoup(FORM_HTML, "html.parser")
    field = soup.find(id=control)
    assert field is not None
    if placement == "missing":
        field.decompose()
    elif placement == "outside":
        soup.append(field.extract())
    elif placement == "other-form":
        soup.find(id="logoutForm").append(field.extract())
    else:
        field["form"] = "logoutForm"
    SERVER_STATE["form"] = str(soup)

    expected = (
        "PORTAL_STRUCTURE_CHANGED"
        if control == "TraineeId" or placement == "explicit-other-owner"
        else "ELEMENT_NOT_FOUND"
    )
    assert_validation_without_post(client, expected_code=expected)


@pytest.mark.parametrize(
    "option, requested",
    [
        ('<option value="4">Final Assessment</option>', "4"),
        ('<option value="4">Ordinary-looking label</option>', "4"),
        ('<option value="9">Final Assessment</option>', "9"),
        ('<option value="9">Final Assessment</option>', "Final Assessment"),
    ],
    ids=["option-four", "option-four-renamed", "final-label-by-value", "final-label-by-text"],
)
def test_final_assessment_never_posts(client, option, requested):
    SERVER_STATE["form"] = FORM_HTML.replace(
        '<option value="3">Simulator</option>', option,
    )
    assert_validation_without_post(client, training_type=requested)


@pytest.mark.parametrize(
    "final_url",
    [
        "/Trainee/TrainingLog?TraineeId=456",
        "/Trainee/TrainingLog?unrelated=123",
        "/Trainee/TrainingLog?TraineeId=123&TraineeId=",
        "/Trainee/TrainingLog?TraineeId=&TraineeId=123",
        "/Trainee/TrainingLog?TraineeId=123&TraineeId=123",
    ],
    ids=["wrong-query-id", "missing-query-id", "extra-blank", "first-blank", "duplicate-id"],
)
def test_final_form_url_must_identify_resolved_trainee(client, final_url):
    SERVER_STATE["form_redirect"] = final_url
    # A valid hidden ID must not mask a wrong or missing ID in the final URL.
    SERVER_STATE["form"] = FORM_HTML.replace("__TID__", "123")
    assert_validation_without_post(client, expected_code="PORTAL_STRUCTURE_CHANGED")


# -- live-gate evidence dump (DSSP_DUMP_DIR) --------------------------------
def test_a_read_dumps_the_raw_list_when_asked(client, tmp_path, monkeypatch):
    """The Stage 10 diagnostic: the operator needs the portal's own HTML to see
    whether the list came back whole and whether the rows parse as assumed."""
    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path))

    client.list_trainees()

    assert "John Doe" in (tmp_path / "trainee-list.html").read_text()


def test_a_submit_dumps_the_response_body_when_asked(client, tmp_path, monkeypatch):
    """The classify-by-text path is the one a crash window leans on: "duplicate"
    is a match on this body, so the real wording is worth capturing verbatim."""
    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path))

    client.submit_training({"id": "456"}, session())

    dumped = (tmp_path / "submit-response-456.html").read_text()
    assert "already logged" in dumped
    metadata = json.loads((tmp_path / "submit-response-456.metadata.json").read_text())
    assert metadata == {
        "status": 200,
        "content_type": "text/html; charset=utf-8",
        "location": None,
    }


@pytest.mark.parametrize("status", [200, 302])
def test_response_capture_allowlists_metadata_without_logging_secrets(
    client, tmp_path, monkeypatch, caplog, status
):
    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path))
    body = '{"IsSuccessful": true, "Message": "SYNTHETIC-PRIVATE-BODY"}'
    headers = {"Set-Cookie": "session=SYNTHETIC-COOKIE; HttpOnly"}
    if status == 302:
        headers["Location"] = "/redirect-target?token=SYNTHETIC-LOCATION-TOKEN"
    SERVER_STATE["post_response"] = (status, body, headers)
    with caplog.at_level(logging.INFO, logger="dssp.portal"):
        result = client.submit_training({"id": "123"}, session())
    assert result["outcome"] == ("confirmed" if status == 200 else "indeterminate")
    metadata_text = (tmp_path / "submit-response-123.metadata.json").read_text()
    assert json.loads(metadata_text) == {
        "status": status,
        "content_type": "application/json",
        "location": headers.get("Location"),
    }
    assert (tmp_path / "submit-response-123.html").read_text() == body
    assert "SYNTHETIC-COOKIE" not in metadata_text
    assert "set-cookie" not in metadata_text.lower()
    for secret in ("SYNTHETIC-PRIVATE-BODY", "SYNTHETIC-COOKIE", "SYNTHETIC-LOCATION-TOKEN"):
        assert secret not in caplog.text
    assert LAST_POST["count"] == 1


@pytest.mark.parametrize("status", [200, 302, 500])
@pytest.mark.parametrize("failure", ["playwright", "decode", "preflight-coded"])
def test_body_read_failure_preserves_metadata_and_worker_reports_possibly_delivered(
    client, tmp_path, monkeypatch, caplog, status, failure
):
    from playwright.sync_api import APIResponse, Error as PlaywrightError
    from app.worker import Worker

    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path))
    SERVER_STATE["post_response"] = (
        status, '{"IsSuccessful": true}', {"Location": "/redirect-target"},
    )
    original_text = APIResponse.text
    errors = {
        "playwright": PlaywrightError("SYNTHETIC-PRIVATE-READ-ERROR"),
        "decode": UnicodeError("SYNTHETIC-PRIVATE-READ-ERROR"),
        "preflight-coded": e.element_not_found("SYNTHETIC-PRIVATE-READ-ERROR"),
    }
    read_attempts = []

    def fail_post_body(response):
        if urlsplit(response.url).path == "/Trainee/LogTraining":
            metadata = json.loads((tmp_path / "submit-response-123.metadata.json").read_text())
            assert metadata == {
                "status": status, "content_type": "application/json", "location": "/redirect-target",
            }
            read_attempts.append(response.url)
            raise errors[failure]
        return original_text(response)

    monkeypatch.setattr(APIResponse, "text", fail_post_body)
    worker = Worker()
    worker.portal = client
    with caplog.at_level(logging.WARNING):
        result = worker.handle({
            "job_id": "synthetic-body-read", "op": "submit_training",
            "trainee": {"id": "123"}, "session": session(),
        })
    assert result["status"] == "error"
    assert result["error_code"] == "CONFIRMATION_UNKNOWN"
    assert result["proves_nothing_submitted"] is False
    assert "may have been delivered" in result["message"]
    assert "do not retry" in result["message"]
    assert "SYNTHETIC-PRIVATE-READ-ERROR" not in caplog.text
    assert not (tmp_path / "submit-response-123.html").exists()
    assert len(read_attempts) == 1
    assert LAST_POST["count"] == 1
    assert not any(path == "/redirect-target" for _, path in REQUESTS)


def test_submission_capture_is_opt_in(client, tmp_path, monkeypatch):
    monkeypatch.delenv("DSSP_DUMP_DIR", raising=False)
    monkeypatch.chdir(tmp_path)
    assert client.submit_training({"id": "123"}, session())["outcome"] == "confirmed"
    assert list(tmp_path.iterdir()) == []
