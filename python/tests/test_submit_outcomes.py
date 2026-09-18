"""Submission tests against a local DSSP-shaped fixture portal.

These exercise the real Playwright stack (headless, throwaway profile) but never
touch the real portal: the client's constants are patched to the fixture server.
"""

import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

import pytest

from app.portal import constants as c
from app.portal import errors as e
from app.portal.client import PortalClient

TRAINEES_HTML = """
<table class="table-checkable"><tbody>
  <tr><td>1</td><td>a</td><td>John Doe</td><td>x</td><td>Class C</td><td></td><td></td><td>2</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog/TraineeId=123">Log</a></td></tr>
  <tr><td>2</td><td>a</td><td>Jane Roe</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog/TraineeId=456">Log</a></td></tr>
  <tr><td>3</td><td>a</td><td>Weird Case</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog/TraineeId=555">Log</a></td></tr>
  <tr><td>4</td><td>a</td><td>Session Case</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog/TraineeId=666">Log</a></td></tr>
  <tr><td>5</td><td>a</td><td>John Doe</td><td>x</td><td>Class B</td><td></td><td></td><td>0</td><td></td><td></td><td></td>
      <td><a href="/Trainee/TrainingLog/TraineeId=789">Log</a></td></tr>
</tbody></table>
"""

FORM_HTML = """
<form action="/post/__TID__" method="post">
  <input type="hidden" name="__RequestVerificationToken" value="TOK"/>
  <input type="text" id="TrainingDate" name="TrainingDate" value="2020-01-01"/>
  <select id="Instructor" name="Instructor">
    <option value="">--</option><option value="10">Alice</option><option value="11">Bob</option>
  </select>
  <select id="TrainingType" name="TrainingType">
    <option value="">--</option><option value="1">Practical</option><option value="2" selected>Theory</option>
  </select>
  <input type="checkbox" name="Agree" checked/>
  <button type="submit" name="submitBtn" value="save">Save</button>
</form>
"""

LAST_POST = {}


class _Handler(BaseHTTPRequestHandler):
    def _send(self, code, body, headers=None):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.startswith("/Trainee/TrainingLog/TraineeId="):
            tid = self.path.rsplit("=", 1)[-1]
            self._send(200, FORM_HTML.replace("__TID__", tid))
        elif self.path.startswith("/Trainee"):
            self._send(200, TRAINEES_HTML)
        else:
            self._send(200, "<html></html>")

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        LAST_POST.clear()
        LAST_POST["path"] = self.path
        LAST_POST["body"] = self.rfile.read(length).decode()
        LAST_POST["content_type"] = self.headers.get("Content-Type")
        LAST_POST["xrw"] = self.headers.get("X-Requested-With")

        tid = self.path.rsplit("/", 1)[-1]
        if tid == "123":
            self._send(302, "", {"Location": "/Trainee?saved=1"})
        elif tid == "456":
            self._send(200, "<html><body>Record already logged</body></html>")
        elif tid == "789":
            self._send(
                200,
                "<html><body><div class='validation-summary-errors'>"
                "<ul><li>Date is invalid</li></ul></div></body></html>",
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


@pytest.fixture()
def client(portal_server, monkeypatch):
    origin = f"http://127.0.0.1:{portal_server.server_address[1]}"
    monkeypatch.setattr(c, "DSSP_ORIGIN", origin)
    monkeypatch.setattr(c, "TRAINEE_PAGE_URL", f"{origin}/Trainee")
    monkeypatch.setattr(
        c, "TRAINEE_LIST_URL", f"{origin}/Trainee?pgsize=10000&page=1&keywords="
    )
    LAST_POST.clear()
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
    assert result["reference"].endswith("/Trainee?saved=1")

    body = LAST_POST["body"]
    assert "TrainingDate=2026-09-14" in body  # 14/09/2026 normalised
    assert "Instructor=10" in body  # label "Alice" resolved to value
    assert "TrainingType=2" in body
    assert "__RequestVerificationToken=TOK" in body
    assert "submitBtn=save" in body
    assert LAST_POST["content_type"] == "application/x-www-form-urlencoded"
    assert LAST_POST["xrw"] == "XMLHttpRequest"


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
