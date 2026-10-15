"""Submission fault boundaries through the real client/worker and fake requests.

No browser or sockets are opened. Reuse the synthetic portal documents from the
loopback integration suite rather than inventing a second form contract.
"""

import logging
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import urljoin, urlsplit

import pytest
from playwright.sync_api import Error as PlaywrightError

from app import protocol as p
from app.portal import client as client_module
from app.portal import constants as c
from app.portal import parsing as parse
from app.portal.client import PortalClient
from app.worker import Worker
from test_submit_outcomes import FORM_HTML, TRAINEES_HTML, session


LOGIN_HTML = '<form id="loginForm" action="/Account/Login"></form>'


class _Response:
    def __init__(self, url, body, *, status=200, headers=None):
        self.url = url
        self.body = body
        self._status = status
        self._headers = headers or {"content-type": "application/json"}
        self.read_error = None
        self.status_error = None
        self.headers_error = None
        self.reads = 0

    @property
    def status(self):
        if self.status_error is not None:
            raise self.status_error
        return self._status

    @property
    def ok(self):
        return 200 <= self.status < 300

    @property
    def headers(self):
        if self.headers_error is not None:
            raise self.headers_error
        return self._headers

    def text(self):
        self.reads += 1
        if self.read_error is not None:
            raise self.read_error
        return self.body


class _FixtureRequests:
    def __init__(self, origin, list_url, form_url):
        self.submit_url = urljoin(origin + "/", c.TRAINING_SUBMIT_PATH)
        self.responses = {
            list_url: _Response(list_url, TRAINEES_HTML),
            form_url: _Response(form_url, FORM_HTML.replace("__TID__", "123")),
        }
        self.post_response = _Response(self.submit_url, '{"IsSuccessful": true}')
        self.get_errors = {}
        self.calls = []
        self.post_options = []

    def _record(self, method, url):
        assert urlsplit(url).scheme == "http"
        assert urlsplit(url).netloc == "127.0.0.1:1"
        self.calls.append((method, url))

    def get(self, url, **options):
        self._record("GET", url)
        assert options == {"timeout": c.NAV_TIMEOUT_MS}
        if url in self.get_errors:
            raise self.get_errors[url]
        return self.responses[url]

    def post(self, url, **options):
        self._record("POST", url)
        assert url == self.submit_url
        self.post_options.append(options)
        return self.post_response


@pytest.fixture
def fixture_client(monkeypatch):
    origin = "http://127.0.0.1:1"
    list_url = f"{origin}/Trainee?pgsize=10000&page=1&keywords="
    monkeypatch.setattr(c, "DSSP_ORIGIN", origin)
    monkeypatch.setattr(c, "TRAINEE_PAGE_URL", f"{origin}/Trainee")
    monkeypatch.setattr(c, "TRAINEE_LIST_URL", list_url)
    monkeypatch.delenv("DSSP_DUMP_DIR", raising=False)
    form_url = c.training_form_url("123")
    requests = _FixtureRequests(origin, list_url, form_url)
    context = SimpleNamespace(request=requests)

    def no_browser_start(portal):
        assert portal._context is context, "fault tests must not start a browser"

    monkeypatch.setattr(PortalClient, "start", no_browser_start)
    worker = Worker()
    worker.portal._context = context
    return worker, requests, list_url, form_url


def _submit(worker, **overrides):
    return worker.handle({
        "job_id": "synthetic-fault", "op": "submit_training",
        "trainee": {"id": "123"}, "session": session(**overrides),
    })


def _assert_one_post(requests, list_url, form_url):
    assert requests.calls == [
        ("GET", list_url), ("GET", form_url), ("POST", requests.submit_url),
    ]
    assert len(requests.post_options) == 1
    assert requests.post_options[0]["max_redirects"] == 0
    assert requests.post_response.reads == 1


@pytest.mark.parametrize("target", ["metadata.json", "html"])
@pytest.mark.parametrize("write_error", [OSError, ValueError], ids=["os-error", "value-error"])
@pytest.mark.parametrize("body, expected", [
    ('{"IsSuccessful": true}', "confirmed"),
    ('{"IsSuccessful": false, "Message": "Date is invalid"}', "rejected"),
    ('{"IsSuccessful": "true"}', "indeterminate"),
    (LOGIN_HTML, "SESSION_EXPIRED"),
], ids=["confirmed", "rejected", "indeterminate", "session-expired"])
def test_response_dump_write_failure_is_best_effort_without_reposting(
    fixture_client, tmp_path, monkeypatch, caplog, target, write_error, body, expected
):
    worker, requests, list_url, form_url = fixture_client
    requests.post_response.body = body
    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path))
    original_write = Path.write_text
    writes = []
    failed_name = f"submit-response-123.{target}"

    def fail_selected_write(path, text, *args, **kwargs):
        writes.append(path.name)
        if path.name == failed_name:
            raise write_error("synthetic response-write failure")
        return original_write(path, text, *args, **kwargs)

    monkeypatch.setattr(Path, "write_text", fail_selected_write)
    with caplog.at_level(logging.WARNING, logger="dssp.portal"):
        result = _submit(worker)

    if expected == "SESSION_EXPIRED":
        assert result["status"] == "error"
        assert result["error_code"] == expected
        assert result["proves_nothing_submitted"] is False
    else:
        assert result["status"] == "ok"
        assert result["outcome"] == expected
        assert result["attempts"] == 1
        if expected != "confirmed":
            assert "reference" not in result
    assert writes == [
        "trainee-list.html", "training-form-123.html",
        "submit-response-123.metadata.json", "submit-response-123.html",
    ]
    assert not (tmp_path / failed_name).exists()
    surviving = "html" if target == "metadata.json" else "metadata.json"
    assert (tmp_path / f"submit-response-123.{surviving}").is_file()
    assert sum("could not dump" in record.message for record in caplog.records) == 1
    _assert_one_post(requests, list_url, form_url)


@pytest.mark.parametrize("stage", ["list", "form"])
@pytest.mark.parametrize("failure, expected_code, proves_unsent", [
    ("request", "NETWORK", False),
    ("body-read", "NETWORK", False),
    ("body-decode", "SUBMISSION_FAILED", False),
    ("http-status", "NETWORK", False),
    ("login-on-http-error", "SESSION_EXPIRED", True),
])
def test_get_fault_stops_preparation_before_any_post(
    fixture_client, stage, failure, expected_code, proves_unsent
):
    worker, requests, list_url, form_url = fixture_client
    url = list_url if stage == "list" else form_url
    response = requests.responses[url]
    if failure == "request":
        requests.get_errors[url] = PlaywrightError("synthetic GET transport failure")
    elif failure == "body-read":
        response.read_error = PlaywrightError("synthetic GET read failure")
    elif failure == "body-decode":
        response.read_error = UnicodeError("synthetic GET decoding failure")
    else:
        response._status = 503
        if failure == "login-on-http-error":
            response.body = LOGIN_HTML

    result = _submit(worker)

    assert result["status"] == "error"
    assert result["error_code"] == expected_code
    # NETWORK and unexpected errors remain conservative, even on a GET.
    assert result["proves_nothing_submitted"] is proves_unsent
    assert requests.calls == (
        [("GET", list_url)] if stage == "list"
        else [("GET", list_url), ("GET", form_url)]
    )
    assert response.reads == (0 if failure == "request" else 1)
    assert requests.post_options == []
    assert requests.post_response.reads == 0


@pytest.mark.parametrize("fault, expected_code, proves_unsent", [
    ("nonstring-date", "SUBMISSION_FAILED", False),
    ("invalid-calendar-date", "VALIDATION_FAILED", True),
    ("type-label-mismatch", "VALIDATION_FAILED", True),
    ("instructor-label-mismatch", "VALIDATION_FAILED", True),
    ("payload-parser", "SUBMISSION_FAILED", False),
    ("payload-encoding", "SUBMISSION_FAILED", False),
])
def test_form_data_faults_are_contained_before_post(
    fixture_client, monkeypatch, fault, expected_code, proves_unsent
):
    worker, requests, list_url, form_url = fixture_client
    overrides = {}
    if fault == "nonstring-date":
        overrides["training_date"] = {"day": 14}
    elif fault == "invalid-calendar-date":
        overrides["training_date"] = "2026-02-30"
    elif fault == "type-label-mismatch":
        overrides["training_type_label"] = "Practical"
    elif fault == "instructor-label-mismatch":
        overrides.update(instructor="10", instructor_label="Bob")
    else:
        def fail_form_data(*args, **kwargs):
            raise ValueError("synthetic form-data failure")

        if fault == "payload-parser":
            monkeypatch.setattr(parse, "build_form_payload", fail_form_data)
        else:
            monkeypatch.setattr(client_module, "urlencode", fail_form_data)

    result = _submit(worker, **overrides)

    assert result["status"] == "error"
    assert result["error_code"] == expected_code
    assert result["proves_nothing_submitted"] is proves_unsent
    assert requests.calls == [("GET", list_url), ("GET", form_url)]
    assert requests.responses[form_url].reads == 1
    assert requests.post_options == []
    assert requests.post_response.reads == 0


@pytest.mark.parametrize("status, body, location, expected", [
    (200, '{"IsSuccessful": "true"}', None, "indeterminate"),
    (200, '{"IsSuccessful": true', None, "indeterminate"),
    (200, '[{"IsSuccessful": true}]', None, "indeterminate"),
    (200, "<p>Training saved successfully.</p>", None, "indeterminate"),
    (200, '{"IsSuccessful": false, "success": true}', None, "rejected"),
    (300, '{"IsSuccessful": true}', "/redirect-target", "indeterminate"),
    (300, '{"IsSuccessful": false}', "/redirect-target", "indeterminate"),
    (300, '<div class="validation-summary-errors">Invalid</div>', None, "indeterminate"),
    (300, '{"IsSuccessful": true}', "/Account/Login", "SESSION_EXPIRED"),
], ids=[
    "nonboolean", "truncated-json", "array-wrapped-success", "success-prose",
    "false-overrides-alias", "300-success", "300-failure", "300-validation",
    "300-login",
])
def test_unsafe_and_redirect_responses_halt_without_followup_requests(
    fixture_client, status, body, location, expected
):
    worker, requests, list_url, form_url = fixture_client
    response = requests.post_response
    response._status = status
    response.body = body
    if location is not None:
        response._headers["location"] = location

    result = _submit(worker)

    if expected == "SESSION_EXPIRED":
        assert result["status"] == "error"
        assert result["error_code"] == "SESSION_EXPIRED"
        assert result["proves_nothing_submitted"] is False
    else:
        assert result["status"] == "ok"
        assert result["outcome"] == expected
        assert result["attempts"] == 1
        assert "reference" not in result
        if status == 300:
            assert "HTTP 300" in result["message"]
    _assert_one_post(requests, list_url, form_url)


@pytest.mark.parametrize("stage", ["status", "headers", "classification", "worker-response"])
def test_worker_contains_unexpected_post_response_error_without_unsent_proof(
    fixture_client, monkeypatch, caplog, stage
):
    worker, requests, list_url, form_url = fixture_client
    failure = RuntimeError(f"synthetic post-response {stage} failure")
    if stage == "status":
        requests.post_response.status_error = failure
    elif stage == "headers":
        requests.post_response._status = 300
        requests.post_response.headers_error = failure
    elif stage == "classification":
        def fail_classification(*args, **kwargs):
            raise failure

        monkeypatch.setattr(parse, "submission_outcome", fail_classification)
    else:
        original_ok = p.ok_response

        def fail_submit_response(job_id, op, **fields):
            if op == "submit_training":
                raise failure
            return original_ok(job_id, op, **fields)

        monkeypatch.setattr(p, "ok_response", fail_submit_response)

    with caplog.at_level(logging.ERROR, logger="dssp.worker"):
        result = _submit(worker)

    assert result["status"] == "error"
    assert result["error_code"] == "SUBMISSION_FAILED"
    assert result["proves_nothing_submitted"] is False
    assert result["job_id"] == "synthetic-fault"
    assert result["op"] == "submit_training"
    assert result["message"] == str(failure)
    assert any(record.exc_info for record in caplog.records)
    _assert_one_post(requests, list_url, form_url)
    assert worker.handle({"job_id": "after-fault", "op": "ping"}) == {
        "v": 1, "job_id": "after-fault", "op": "ping", "status": "ok", "pong": True,
    }
    _assert_one_post(requests, list_url, form_url)
