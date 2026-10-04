# One ordinary-session live verification — authorization required

**Prepared locally; no live request has been made. Stages 15 and 24 remain
Blocked. This document is not authorization to run a submission.**

Never repeat Joshua's existing session. Use only a genuinely needed, unsubmitted
ordinary session, explicitly approved for this one run. Do not invent training
activity to test the bot. A screenshot of an existing record is not POST evidence.

## Final diff review

| Area | Reviewed behavior |
| --- | --- |
| Request | POST `/Trainee/LogTraining`; `application/x-www-form-urlencoded; charset=UTF-8`, not JSON. `urlencode` encodes four nested `LogDetails[...]` fields: trainee, date, instructor, training option. |
| Empty assessments | jQuery emits no entries for an empty `FinalAssessments` array. No assessment controls, unrelated form fields, or logout token are serialized. |
| Trainee identity | The final GET URL must have the configured origin/path and exactly one matching `TraineeId` value. The form must contain exactly one enabled hidden `TraineeId` control, with matching id/name, ownership, and value. The resolved record supplies the outgoing ID. |
| Controls | Required controls are searched within the unique `form#frmtraininglog`; disabled/misowned controls are refused. Existing selector alternatives remain. |
| Final assessment | Option `4`, a final-assessment label, or a nonempty assessment payload is refused before POST. Final-assessment submission is not supported. |
| Confirmation | Only a boolean true recognized flag on HTTP 2xx can confirm. `IsSuccessful` takes precedence over `success`/`Success`; false rejects. Login checks take priority; empty bodies, generic text, 3xx success flags, and reloads do not confirm. |
| Redirects | `max_redirects=0`: no POST redirect is followed or replayed. A login `Location` reports session expiry without fetching it; other redirects remain unconfirmed. |
| Retries | Python has one POST call and no retry loop. Playwright's request retry default is zero. Rust's single-run loop can retry provably-unsent transient failures (such as missing controls), not indeterminate outcomes or possibly delivered NETWORK/TIMEOUT failures. |
| Evidence | Opt-in body dump plus an allowlisted metadata sidecar: status, Content-Type, Location only. No Cookie, Set-Cookie, Authorization, or request headers are collected by the sidecar. |

The core single-run path has **no checkpoint**. The one-shot marker in the launch
command below prevents accidental reuse of that procedure, but cannot prevent
someone bypassing it with another command. Never remove the marker to retry.

## Prerequisites

1. Explicit operator approval for the exact trainee ID, date, instructor, and
   ordinary training type, including permission for one real POST. Capture review
   acknowledges that the HTML establishes the client contract but a live POST
   response is still unverified. Do not run while authorization is pending.
2. Review the entire local change set, including response metadata capture;
   approve/record the tested revision according to the commit workflow. The older
   confirmation-only commit alone does not contain the AJAX correction.
3. Offline suites must be green. Existing Python venv, Playwright Chromium,
   Rust toolchain, cached Cargo dependencies, and a desktop/display must work.
4. No other bot/extension automation, worker, or browser process may use the same
   persistent profile concurrently. Disable legacy extension automation.
5. In an operator-authorized read-only portal review, identify the trainee by ID
   and inspect the full training history. Save private before evidence and confirm
   the intended date/type/instructor combination is absent. If uncertain, stop.
   Do not rely solely on the trainee name or a total session count.
6. Obtain exact instructor/type option values and retain their displayed labels
   privately. For this procedure choose daily classroom or daily practical
   training (observed options `1` or `2`), not final assessment. Replace the
   example's date with the genuinely intended date; use `YYYY-MM-DD`.
7. Use a fresh private directory below the gitignored `python/.evidence/`.
   No HAR, Playwright trace, debug logging, HTTP proxy recording, shell `set -x`,
   cookies, passwords, or authorization headers belong in shared logs.

## Offline preparation — from the repository root

These commands do not contact the portal. Run the entire block as written:
the subshell stops on failure, and directory creation deliberately fails if this
run directory already exists. The copy is also no-clobber. Do not overwrite an
existing job, remove an attempt marker, or delete previous evidence to prepare
another run. If the directory already exists, stop and review it without making
changes.

```sh
(
    set -eu
    umask 077
    mkdir -p python/.evidence
    mkdir -m 700 python/.evidence/live-single
    cp -n job.example.json python/.evidence/live-single/job.json
    chmod 600 python/.evidence/live-single/job.json
    cargo build --offline --manifest-path rust/Cargo.toml --target-dir rust/target
)
```

Only after successful creation of a fresh run directory, edit the new
`python/.evidence/live-single/job.json` locally. Do not edit an existing run's
job or attempt marker. Use exactly the example's
single `trainee` object with an explicit ID and its `session` object; no
`trainees` array. Fill all four values with the approved record. Do not put
credentials in the job. Save private before-history evidence and approval notes
in the same directory. Do not paste that job or evidence into chat or a PR.

## Exact launch command — ONLY after separate live authorization

Run once from the repository root. This invokes the Rust single-trainee path,
which invokes the Python worker. Do not run a separate Python submission for
Stage 15: the same chain can provide evidence for both gates without a second
record. Sign in manually in the headed Chromium window if requested.

Job/build validation uses explicit checks and `SystemExit`, not assertions;
`PYTHONOPTIMIZE=1` cannot disable it. Invalid jobs stop before marker creation or
starting Rust. The launcher records an attempt marker **before** starting Rust and refuses
another launch if it exists. It does not retry Rust. Raw stdout/stderr are kept
in memory and are not printed or saved as logs, because portal error messages
could contain sensitive data. Only a fixed classification and exit code are
saved in the receipt. Do not add `tee`, shell tracing, or debug environment flags.

```sh
umask 077
python/.venv/bin/python - <<'PY'
import json
import os
import subprocess
from datetime import date, datetime, timezone
from pathlib import Path

root = Path.cwd()
run = root / "python/.evidence/live-single"
job_path = run / "job.json"
try:
    job = json.loads(job_path.read_text())
except (OSError, ValueError):
    raise SystemExit("Cannot read the private job. No process started.")
if not isinstance(job, dict) or set(job) != {"trainee", "session"}:
    raise SystemExit("Use a single-trainee job only. No process started.")
trainee = job["trainee"]
if not isinstance(trainee, dict) or set(trainee) != {"id"}:
    raise SystemExit("Use an explicit trainee ID. No process started.")
tid = trainee["id"]
if not isinstance(tid, str) or not tid.isascii() or not tid.isdecimal():
    raise SystemExit("Invalid trainee ID. No process started.")
session = job["session"]
if not isinstance(session, dict) or set(session) != {"training_date", "instructor", "training_type"}:
    raise SystemExit("Unexpected session fields. No process started.")
if not all(isinstance(value, str) and value.strip() for value in session.values()):
    raise SystemExit("Session values must be nonempty strings. No process started.")
if not session["instructor"].isascii() or not session["instructor"].isdecimal():
    raise SystemExit("Use the verified instructor ID. No process started.")
if session["training_type"] not in ("1", "2"):
    raise SystemExit("Use an approved daily ordinary session. No process started.")
try:
    canonical_date = date.fromisoformat(session["training_date"]).isoformat()
except ValueError:
    raise SystemExit("Use a real training date in YYYY-MM-DD format. No process started.")
if canonical_date != session["training_date"]:
    raise SystemExit("Use YYYY-MM-DD format. No process started.")
binary = root / "rust/target/debug/dssp-bot-core"
if not binary.is_file() or not os.access(binary, os.X_OK):
    raise SystemExit("Build the executable offline first. No process started.")

with (run / "attempt.started").open("x") as marker:
    marker.write(datetime.now(timezone.utc).isoformat() + "\n")
    marker.flush()
    os.fsync(marker.fileno())
(run / "http").mkdir(mode=0o700)
env = os.environ.copy()
for key in list(env):
    if key.startswith("DSSP_") or key in ("DEBUG", "DEBUG_FILE", "PWDEBUG", "RUST_LOG", "PYTHONPATH", "PYTHONSTARTUP"):
        env.pop(key, None)
env.update({
    "DSSP_ORIGIN": "https://dssp.frsc.gov.ng",
    "DSSP_HEADLESS": "0",
    "DSSP_PROFILE_DIR": str(root / "python/.pw-profile"),
    "DSSP_WORKER_PY": str(root / "python/.venv/bin/python"),
    "DSSP_DUMP_DIR": str(run / "http"),
})
print("One authorized run starting; sign in in Chromium if needed. Do not repeat this command.")
try:
    result = subprocess.run(
        [str(root / "rust/target/debug/dssp-bot-core"), str(job_path)],
        cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
except (OSError, KeyboardInterrupt):
    print("Run interrupted or could not complete. Keep the marker; inspect the portal. Do not retry.")
    raise SystemExit(3)
classification = "unconfirmed"
if result.returncode == 0:
    if any(line.startswith("RESULT: confirmed") for line in result.stdout.splitlines()):
        classification = "confirmed"
    elif any(line.startswith("RESULT: duplicate") for line in result.stdout.splitlines()):
        classification = "duplicate"
elif result.returncode == 4:
    classification = "failed"
receipt = {
    "exit_code": result.returncode,
    "classification": classification,
    "finished_at_utc": datetime.now(timezone.utc).isoformat(),
}
(run / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
print("Run ended:", classification, "exit", result.returncode)
print("Inspect private response evidence and the exact training-history record. Do not rerun.")
raise SystemExit(result.returncode if 0 <= result.returncode <= 255 else 3)
PY
```

If this command fails or is interrupted at any point after creating the marker,
**leave it in place**. Lack of a receipt or a dump does not prove that no POST
was sent. Do not delete the directory, remove the marker, restart the worker,
or use another job file to retry. Investigate first.

## Response evidence — inspect locally, never print into shared logs

Under `python/.evidence/live-single/http/`:

- `submit-response-<id>.metadata.json`: `status`, `content_type`, and `location`
  (null if absent), saved before attempting to read the response body. This
  describes the direct response; redirects are not followed. No other response
  headers are copied. Body-read failure is `CONFIRMATION_UNKNOWN`, explicitly
  possibly delivered, and must halt without another POST.
- `submit-response-<id>.html`: the response text, despite the `.html` extension;
  it can contain JSON or HTML. Inspect locally for boolean `IsSuccessful` and
  `Message`, but do not copy the raw body into a report.
- GET dumps may include trainee data and the logout form's antiforgery token.
  They are private evidence, not safe logs.

The body and Location can themselves contain sensitive data. They are stored
verbatim for private inspection, not guaranteed redacted. `umask 077` protects
new files and the directory is mode 700. Never upload these files, export all
headers, or paste them into chat/PRs. In a shared report record only status,
media type, whether Location was present (and a redacted path if safe), the
boolean result, exit code, and the history-check conclusion.

Dump writes are best-effort. Missing, unreadable, or incomplete evidence blocks
verification even if the CLI reports success. If the POST times out or its body
cannot be read, there may be no complete dump: stop and inspect history, never
repeat the POST to obtain a better capture. Browser-tab Network panels do not
necessarily show `BrowserContext.request` traffic; use these worker captures,
not an assumption that opening Developer Tools captures the worker's POST.

## Stop rules

| Observed result | Action |
| --- | --- |
| `confirmed`, exit 0 | Still inspect metadata and exact history record; not an automatic gate pass. |
| `duplicate`, exit 0 | Stop. It does not prove a new successful submission and cannot satisfy this verification. Reconcile the preflight history check. |
| Indeterminate, unknown output, exit 3, timeout, crash, missing receipt | Stop; no retry. Inspect the portal read-only and reconcile the exact record. |
| Rejected / exit 4 | Stop and inspect response plus history; no repeat under this authorization. |
| Exit 1 or 2 | Session/setup/job failure; stop and investigate. A new invocation requires review and separate authorization, not deleting the marker. |
| Any 3xx, including a success flag | No confirmation. Login Location means expired session; otherwise indeterminate. Never follow or replay the POST manually. |

Rust may repeat a *pre-submission* operation if it proves nothing was submitted;
this is not permission to repeat a delivered POST. The reviewed policy halts on
unconfirmed results and possibly delivered failures. Do not wrap the launcher
in any retry loop, supervisor restart, or scheduled task.

## Verify the exact record in training history

After the run, inspect history read-only (with separate read-only authorization
if the agent is to do it). If using the same persistent browser profile, wait
for the worker to close it first; do not open competing profile processes.

1. Open `/Trainee/TrainingLog?TraineeId=...` for the **approved ID**. Check the ID
   in the URL and the displayed identity; a similar name is insufficient.
2. Compare against the saved before-history evidence. Require exactly one new
   row matching the intended **training date, training-type label, instructor**,
   and duration where shown. Confirm the portal's date convention; do not guess
   ambiguous day/month order from a screenshot.
3. Check `Date Logged` falls within the run window, accounting for portal versus
   local/UTC timezone, and `Logged By` is the authorized operator. Match option
   values in the private job to the displayed labels verified before the run.
4. Save a private after screenshot/page and a short reconciliation note. A higher
   total count or a generic success banner is not enough. Missing, multiple, or
   mismatched rows remain unresolved. Do not press “Log Training” during this check.
5. If history proves a record exists after an indeterminate response, record that
   reconciliation but keep the automated confirmation gate blocked. Fix or
   investigate confirmation offline; do not submit the same record again.

## Gate acceptance

Keep Stages 15/24 Blocked until all of the following are recorded and reviewed:

- Approved genuinely needed session, demonstrably absent before the run.
- Tested code revision and private job reference.
- Direct POST status, Content-Type, body, and Location presence/absence captured.
- HTTP 2xx with an explicit boolean success flag recognized by the parser.
- Rust single-chain receipt with `confirmed` and exit 0.
- Exactly one matching new history row, verified against before evidence.
- No unresolved timeout, capture failure, identity mismatch, or duplicate.

The same run can substantiate Gate 1 (Python → portal → confirmation) and Gate 2
(Rust → Python → portal → Rust). It does not verify batch checkpoint/recovery.
Publish only a sanitized stage record; raw evidence stays private and gitignored.
