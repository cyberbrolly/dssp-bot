# Stage 32 — offline failure-boundary acceptance

Status: **Passed for the matrix below**, using synthetic jobs, fake portal
requests, fixture worker subprocesses, loopback HTTP, and mocked extension fetch.
No live portal request, real training submission, private job/evidence access,
or attempt-marker change is part of these checks. Stage 33 browser integration
and live batch/recovery/final-assessment acceptance are not established here.

## Failure matrix

| Boundary / injected failure | Required result | Regression source |
| --- | --- | --- |
| Missing/non-executable worker, invalid supervision configuration | Refuse before worker execution; no submit | `rust/tests/daemon_cli.rs` |
| Invalid/malformed/missing-version/wrong-version ready line; startup EOF or silence | Fail admission/startup with zero submits; no worker replacement or retry | `rust/tests/daemon_cli.rs`, `rust/src/worker.rs` |
| Malformed submit JSON; wrong/missing version, job ID or operation; unexpected status/ready; EOF | Indeterminate / CLI halt 3; exactly one submit, no later trainees | `rust/tests/daemon_cli.rs` |
| Returned explicit trainee ID differs from requested ID | Reject response; retain requested identity/provenance, never accept it as another trainee's success | `rust/tests/daemon_cli.rs` |
| Missing/unrecognized submission outcome | Indeterminate, no automatic retry | `rust/tests/daemon_cli.rs` |
| Silent or late submit reply | Actual response deadline fires; client becomes unusable, late reply cannot satisfy another request | `rust/src/worker.rs`, `rust/tests/daemon_cli.rs` |
| Worker ignores EOF during shutdown | Grace expires; kill/reap direct fixture child, no replay; daemon polling remains available | `rust/src/worker.rs`, `rust/tests/daemon_cli.rs` |
| Network/timeout/confirmation-unknown/other errors with false or missing delivery proof | Halt possibly delivered work; no additional submit | `rust/tests/daemon_cli.rs`, existing decision tests |
| Session expiry / structure change, including true delivery proof | Halt rather than retry | `rust/tests/daemon_cli.rs` |
| Provably-unsent transient preflight failure | Only existing bounded retry policy; success or exhaustion after three attempts | `rust/tests/daemon_cli.rs` |
| Explicit rejection | Accurate failed result; continue the next trainee without retrying the rejected row | `rust/tests/daemon_cli.rs` |
| Fresh start after unsafe failure / daemon restart | Checkpoint refusal 409 before worker creation; requested and in-flight provenance preserved | `rust/tests/daemon_cli.rs`, existing daemon/recovery tests |
| Unreadable/corrupt checkpoint; failed pre-send, pause/resume or stop persistence | Refuse or halt before later submits; preserve last durable evidence | Existing daemon/engine/store tests |
| Metadata dump or body dump write fails independently | Best-effort capture must not change classification or cause another POST | `python/tests/test_portal_faults.py` |
| List/form GET transport, body-read/decode, HTTP or login failure | Zero POSTs | `python/tests/test_portal_faults.py` |
| Form parse, validation or encoding fails | Zero POSTs | `python/tests/test_portal_faults.py` |
| Unsafe HTTP 200 JSON/body or non-login 3xx | No text/empty/redirect confirmation; no redirect follow/replay | `python/tests/test_portal_faults.py`, existing parsing/submission tests |
| Body-read or unexpected post-response exception, including misleading preflight expiry exception | False delivery proof; one POST only; worker remains responsive | `python/tests/test_portal_faults.py`, `python/tests/test_submit_outcomes.py` |
| UI/control errors, disconnected/stale views, absent reports, malformed API replies | No admission/control auto-retry, engine fallback, or fabricated terminal success | Existing daemon client/popup/view/worker TypeScript tests |

## Bugs demonstrated and fixed

Before the fixes, two new subprocess regressions showed the daemon incorrectly
completing both trainees after accepting a submit reply with either:

- `op: get_form_options` rather than the requested submit operation; or
- a returned trainee ID different from the explicitly requested ID.

`WorkerClient` now validates both response echoes (`job_id`, `op`) and, when both
are present, explicit requested versus returned trainee ID. Omitted trainee
payloads and name-selector resolution remain compatible. No portal response
contract, duplicate heuristic, or name-alias assumption was added.

A transport/protocol failure invalidates the worker client and closes its stdin
and response receiver. Further sends fail before writing, including after a late
reply. Mid-submit failures remain possibly delivered/indeterminate, not retryable.

## Worker supervision configuration

These settings are read by Rust before worker spawn, in CLI and daemon modes:

| Variable | Default | Meaning |
| --- | ---: | --- |
| `DSSP_WORKER_READY_TIMEOUT_MS` | `10000` | Wait for startup ready line |
| `DSSP_WORKER_RESPONSE_TIMEOUT_MS` | `360000` | Wait for one response after writing a request |
| `DSSP_WORKER_SHUTDOWN_TIMEOUT_MS` | `2000` | EOF grace before killing/reaping the direct worker |

Overrides must be positive integer milliseconds and fit a representable deadline;
invalid values refuse worker creation. The response default accommodates the
existing five-minute manual-login wait. If login/navigation limits are increased,
review this response limit too; timeout never grants permission to repeat a POST.

## Validation results

- `cargo fmt --check`: passed.
- `cargo test --offline`: **250 passed** (216 coordinator/unit/loopback and
  34 binary/CLI tests). Stage 32 adds 35 tests to the Stage 31 baseline.
- `cargo clippy --offline --all-targets -- -D warnings`: passed.
- `.venv/bin/python -m pytest -q` from `python/`: **356 passed**, including
  48 new fault cases.
- `pnpm typecheck`: passed; `pnpm test`: **313 passed** across 20 files.
- `git diff --check`: passed.

All binary tests explicitly select the offline fixture worker, clear inherited
configuration, and isolate profiles, checkpoints, synthetic jobs and traces in
temporary directories. Python fault cases do not start a browser; the existing
submission integration tests use a loopback-only fixture portal.

## Remaining limits — not acceptance claims

- Stdin writes/flush remain synchronous without a deadline. The response timer is
  not an end-to-end job deadline and does not cover a blocked pipe write.
- Killing/reaping the direct worker does not guarantee browser-descendant cleanup,
  profile-lock release, or cancellation/rollback of a delivered POST. OS kill/reap
  failures are logged, not treated as proof of successful cleanup.
- Reader threads are not joined during shutdown: descendants retaining pipe handles
  can keep them alive. Protocol line sizes and queued output are not bounded by
  this change; hostile output/resource-exhaustion hardening remains security work.
- No cross-process ownership lock or installed-browser/Firefox verification was
  added. Daemon connection saturation can still affect polling availability.
- Passing this matrix does not authorize a live run or establish live batch safety,
  recovery, duplicate-response semantics, or final-assessment support.
