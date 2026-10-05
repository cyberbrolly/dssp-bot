"""Worker stdio protocol tests — real subprocess, no browser involved."""

import json
import subprocess
import sys

import pytest

from app import protocol as p
from app.portal import errors as e
from pathlib import Path

PY_ROOT = Path(__file__).resolve().parent.parent


def run_worker(requests, timeout=60):
    """Feed request lines to a fresh worker; return parsed stdout + process."""
    proc = subprocess.run(
        [sys.executable, "-m", "app.worker"],
        cwd=PY_ROOT,
        input="\n".join(requests) + "\n",
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    # json.loads on every stdout line also proves stdout is protocol-only.
    lines = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
    return lines, proc


@pytest.mark.parametrize("attempted", [False, True])
def test_session_expiry_protocol_hint_tracks_submission_attempt(attempted):
    error = e.session_expired(submission_attempted=attempted)
    assert error.submission_attempted is attempted
    response = p.error_response(
        "expiry-test", "submit_training", error.error_code, error.message,
        submission_attempted=error.submission_attempted,
    )
    assert response["error_code"] == "SESSION_EXPIRED"
    assert response["proves_nothing_submitted"] is (not attempted)


def test_default_pre_submission_expiry_and_confirmation_unknown_flags():
    assert e.session_expired().submission_attempted is False
    assert e.confirmation_unknown("Unverified response").submission_attempted is True
    assert p.error_response(
        "expiry-test", "submit_training", "SESSION_EXPIRED", "Session expired",
    )["proves_nothing_submitted"] is True


def test_ping_roundtrip_and_stdout_is_protocol_only():
    lines, proc = run_worker(['{"v":1,"job_id":"t1","op":"ping"}'])
    assert proc.returncode == 0
    assert lines[0] == {"v": 1, "status": "ready"}

    ping = next(line for line in lines if line.get("job_id") == "t1")
    assert ping["status"] == "ok"
    assert ping["pong"] is True
    assert ping["op"] == "ping"


def test_unknown_op_is_rejected():
    lines, _ = run_worker(['{"v":1,"job_id":"a","op":"nope"}'])
    response = next(line for line in lines if line.get("job_id") == "a")
    assert response["status"] == "error"
    assert response["error_code"] == "UNKNOWN_OP"
    assert response["proves_nothing_submitted"] is True


def test_invalid_json_is_rejected():
    lines, _ = run_worker(["not-json"])
    bad = [line for line in lines if line.get("error_code") == "BAD_REQUEST"]
    assert bad
    assert bad[0]["proves_nothing_submitted"] is True


def test_submit_validation_rejects_missing_fields_without_browser():
    lines, _ = run_worker(
        ['{"v":1,"job_id":"s","op":"submit_training","trainee":{},"session":{}}']
    )
    response = next(line for line in lines if line.get("job_id") == "s")
    assert response["status"] == "error"
    assert response["error_code"] == "BAD_REQUEST"
