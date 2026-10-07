"""Test only the documented launcher's validation prefix, never its live command."""

import ast
import copy
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
GOOD_JOB = {
    "trainee": {"id": "123"},
    "session": {
        "training_date": "2026-10-01",
        "instructor": "10",
        "training_type": "1",
    },
}


def launcher_tree():
    document = (REPO_ROOT / "docs/live-single-verification.md").read_text()
    blocks = re.findall(r"```sh\n(.*?)\n```", document, re.S)
    launch = next(block for block in blocks if "<<'PY'" in block)
    source = launch.split("<<'PY'\n", 1)[1].rsplit("\nPY", 1)[0]
    return ast.parse(source)


def validation_code():
    # Stop before the first with statement: the exclusive attempt-marker write.
    # No marker, browser, worker, or Rust code is included in this test program.
    prefix = []
    for node in launcher_tree().body:
        if isinstance(node, ast.With):
            break
        prefix.append(node)
    else:
        pytest.fail("Documented launcher no longer has the expected marker boundary.")
    return ast.unparse(ast.Module(body=prefix, type_ignores=[]))


def test_documented_launcher_uses_explicit_checks_not_assertions():
    tree = launcher_tree()
    assert not any(isinstance(node, ast.Assert) for node in ast.walk(tree))
    assert "subprocess.run(" not in validation_code()
    assert '.open("x")' in ast.unparse(tree) or ".open('x')" in ast.unparse(tree)


BAD_JOBS = [
    None,
    [],
    {},
    {**GOOD_JOB, "trainees": [{"id": "123"}]},
    {**GOOD_JOB, "trainee": None},
    {**GOOD_JOB, "trainee": []},
    {**GOOD_JOB, "trainee": {}},
    {**GOOD_JOB, "trainee": {"id": 123}},
    {**GOOD_JOB, "trainee": {"id": ""}},
    {**GOOD_JOB, "trainee": {"id": "１２３"}},
    {**GOOD_JOB, "trainee": {"id": "123", "name": "Synthetic Person"}},
    {**GOOD_JOB, "session": None},
    {**GOOD_JOB, "session": []},
    {**GOOD_JOB, "session": {}},
]
for field, value in (
    ("instructor", ""),
    ("instructor", "Synthetic Instructor"),
    ("instructor", "１０"),
    ("training_type", "4"),
    ("training_type", 1),
    ("training_type", ""),
    ("training_date", "2026-02-30"),
    ("training_date", "20261001"),
    ("training_date", "01/10/2026"),
):
    job = copy.deepcopy(GOOD_JOB)
    job["session"][field] = value
    BAD_JOBS.append(job)
job = copy.deepcopy(GOOD_JOB)
job["session"]["FinalAssessments"] = [{"MarkObtained": 8}]
BAD_JOBS.append(job)


def prepare_private_synthetic_run(tmp_path, job):
    run = tmp_path / "python/.evidence/live-single"
    run.mkdir(parents=True)
    if job != "missing":
        (run / "job.json").write_text(
            "not JSON" if job == "invalid-json" else json.dumps(job)
        )
    marker = run / "attempt.started"
    marker.write_text("SYNTHETIC_EXISTING_MARKER")
    binary = tmp_path / "rust/target/debug/dssp-bot-core"
    binary.parent.mkdir(parents=True)
    binary.write_text("SYNTHETIC_BINARY_NEVER_EXECUTED")
    binary.chmod(0o700)
    return run, marker, binary


def check_validation(tmp_path, optimize):
    env = {**os.environ, "PYTHONOPTIMIZE": optimize}
    return subprocess.run(
        [sys.executable, "-c", validation_code()],
        cwd=tmp_path, env=env, capture_output=True, text=True, timeout=15,
    )


@pytest.mark.parametrize("optimize", ["0", "1"])
@pytest.mark.parametrize("job", BAD_JOBS + ["missing", "invalid-json"])
def test_invalid_job_stops_before_rust_or_marker_even_when_optimized(tmp_path, job, optimize):
    run, marker, _ = prepare_private_synthetic_run(tmp_path, job)
    before = {path.name: path.read_bytes() for path in run.iterdir()}
    result = check_validation(tmp_path, optimize)
    assert result.returncode != 0
    assert "No process started." in result.stderr
    assert {path.name: path.read_bytes() for path in run.iterdir()} == before
    assert marker.read_text() == "SYNTHETIC_EXISTING_MARKER"


@pytest.mark.parametrize("optimize", ["0", "1"])
@pytest.mark.parametrize("build", ["missing", "not-executable"])
def test_invalid_build_stops_before_marker_even_when_optimized(tmp_path, build, optimize):
    run, marker, binary = prepare_private_synthetic_run(tmp_path, GOOD_JOB)
    if build == "missing":
        binary.unlink()  # This is only a disposable synthetic file in tmp_path.
    else:
        binary.chmod(0o600)
    result = check_validation(tmp_path, optimize)
    assert result.returncode != 0
    assert "Build the executable offline first. No process started." in result.stderr
    assert marker.read_text() == "SYNTHETIC_EXISTING_MARKER"
    assert not (run / "http").exists()


@pytest.mark.parametrize("optimize", ["0", "1"])
def test_valid_job_passes_validation_prefix_only_without_launching(tmp_path, optimize):
    run, marker, _ = prepare_private_synthetic_run(tmp_path, GOOD_JOB)
    result = check_validation(tmp_path, optimize)
    assert result.returncode == 0
    assert marker.read_text() == "SYNTHETIC_EXISTING_MARKER"
    assert not (run / "http").exists()
