#!/usr/bin/env python3
"""Offline stdio worker fixture. Never imports app.worker or opens a network."""
import json
import os
from pathlib import Path
import sys
import time

fixtures = json.loads(Path(__file__).with_name("responses.json").read_text())
scenario = json.loads(os.environ.get("DSSP_FIXTURE_SCENARIO", "{}"))
if "DSSP_FIXTURE_STARTS" in os.environ:
    with open(os.environ["DSSP_FIXTURE_STARTS"], "a") as starts:
        starts.write("started\n")
def event(name):
    if "DSSP_FIXTURE_EVENTS" in os.environ:
        with open(os.environ["DSSP_FIXTURE_EVENTS"], "a") as events:
            events.write(name + "\n")


def finish():
    event("stdin_eof")
    if scenario.get("shutdown") == "hang":
        # Finite fixture safety net; the supervisor must kill well before this.
        time.sleep(30)


startup = scenario.get("startup")
if startup == "hang":
    sys.stdin.read()
    finish()
    sys.exit(0)
if startup == "early_eof":
    sys.exit(0)
ready = {"v": 1, "status": "ready"}
if startup == "invalid_ready":
    ready["status"] = "ok"
elif startup == "missing_version":
    del ready["v"]
elif startup == "wrong_version":
    ready["v"] = 2
print("not JSON" if startup == "malformed_json" else json.dumps(ready), flush=True)
submit_count = 0
for line in sys.stdin:
    request = json.loads(line)
    with open(os.environ["DSSP_FIXTURE_TRACE"], "a") as trace:
        trace.write(json.dumps(request) + "\n")
    response = dict(fixtures.get(request["op"], {"v": 1, "status": "ok", "pong": True}))
    response["job_id"] = request["job_id"]
    response["op"] = request["op"]
    if request["op"] == "submit_training":
        time.sleep(float(os.environ.get("DSSP_FIXTURE_DELAY", "0")))
        trainee = request["trainee"]
        ident = trainee.get("id", "fixture-1")
        response["trainee"] = {"id": ident, "name": "Fixture One" if ident == "fixture-1" else "Fixture Two"}
        submit_count += 1
        if "DSSP_FIXTURE_SUBMISSIONS" in os.environ and Path(os.environ["DSSP_CHECKPOINT"]).exists():
            # Observe the durable in-flight window from the other process.
            checkpoint = json.loads(Path(os.environ["DSSP_CHECKPOINT"]).read_text())
            with open(os.environ["DSSP_FIXTURE_SUBMISSIONS"], "a") as snapshots:
                snapshots.write(json.dumps({"request": request, "checkpoint": checkpoint}) + "\n")
        mode = scenario.get("submit")
        if submit_count <= scenario.get("failures", 1):
            if mode == "hang":
                sys.stdin.read()
                finish()
                sys.exit(0)
            if mode == "delayed_reply":
                time.sleep(scenario.get("reply_delay", 0.8))
                event("late_reply")
            if mode == "early_eof":
                sys.exit(0)
            if mode == "malformed_json":
                print("not JSON", flush=True)
                continue
            if mode == "wrong_version":
                response["v"] = 2
            elif mode == "missing_version":
                del response["v"]
            elif mode == "wrong_job_id":
                response["job_id"] = "not-the-request-job"
            elif mode == "wrong_op":
                response["op"] = "get_form_options"
            elif mode == "missing_op":
                del response["op"]
            elif mode == "wrong_trainee_id":
                response["trainee"]["id"] = "not-the-request-trainee"
            elif mode == "missing_trainee":
                del response["trainee"]
            elif mode == "missing_outcome":
                del response["outcome"]
            elif mode == "unknown_outcome":
                response["outcome"] = "not-a-v1-outcome"
            elif mode == "wrong_status":
                response["status"] = "not-a-v1-status"
            elif mode == "ready_echo":
                response["status"] = "ready"
            elif mode == "bare_ready":
                response = {"v": 1, "status": "ready"}
            elif mode == "rejected":
                response["outcome"] = "rejected"
                response["message"] = "synthetic rejection"
            elif mode == "error":
                response["status"] = "error"
                del response["outcome"]
                response["error_code"] = scenario["error_code"]
                response["message"] = "synthetic worker error"
                if "proof" in scenario:
                    response["proves_nothing_submitted"] = scenario["proof"]
    print(json.dumps(response), flush=True)
finish()
