#!/usr/bin/env python3
"""Offline stdio worker fixture. Never imports app.worker or opens a network."""
import json
import os
from pathlib import Path
import sys
import time

fixtures = json.loads(Path(__file__).with_name("responses.json").read_text())
print(json.dumps({"v": 1, "status": "ready"}), flush=True)
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
    print(json.dumps(response), flush=True)
