# DSSP-Bot — Migration Runbook

Executable migration plan for the DSSP-Bot coding agent. Work through the
stages **in order**; verify each stage before continuing. The agent MUST update
the status table below after completing each stage (and after every live gate
run).

## Agreed Decisions

1. This runbook lives at `docs/migration-runbook.md`; the status table is
   maintained here.
2. The Python worker entrypoint is `python -m app.worker` (no `app.main` shim
   is added; the runbook matches the actual architecture).
3. Python verification is formalized as a permanent pytest suite in
   `python/tests/` (the earlier fixture scripts became regression tests).
4. Live Gate 1 runs first: stages 10 → 15 → 24 are cleared against the real
   DSSP portal before building deeper stages.
5. Batch CLI wiring is folded into **Stage 25** and must be finished before
   checkpointing (Stage 27).
6. The legacy TypeScript implementation is **not removed** until Stage 34.

## Stage Status Tracking

|  # | Stage                     | Status        | Tests / Verification            | Notes |
| -: | ------------------------- | ------------- | ------------------------------- | ----- |
|  1 | Inspect repository        | 🟢 Passed     | git status / branch / files     |       |
|  2 | Create migration branch   | 🟢 Passed     | `migration/python-rust` pushed  |       |
|  3 | Create project structure  | 🟢 Passed     | `python/ rust/ docs/`           | `extension/` still absent (Stage 29) |
|  4 | Set up Python             | 🟢 Passed     | venv 3.12.3 + deps + Chromium   | playwright 1.62, bs4 4.15, lxml 6.1.3 |
|  5 | Minimal Python worker     | 🟢 Passed     | `python -m app.worker` + ping   | entrypoint is `app.worker` (decision 2) |
|  6 | Test Playwright           | 🟢 Passed     | Chromium launch smoke           | headed + persistent context |
|  7 | Browser manager           | 🟢 Passed     | `PortalClient` lifecycle        | persistent profile |
|  8 | DSSP portal client        | 🟢 Passed     | portal module checks            | fixture-verified |
|  9 | Connect Playwright → DSSP | 🟢 Passed     | DSSP-shaped fixture navigation  | real portal in Stage 10 |
| 10 | Verify DSSP session       | 🟡 Blocked    | session test                    | awaiting live operator run |
| 11 | Retrieve trainees         | 🟢 Passed     | trainee retrieval               | fixture-verified |
| 12 | Trainee matching          | 🟢 Passed     | `pytest`                        | permanent suite in `python/tests/` |
| 13 | Training form             | 🟢 Passed     | form workflow                   | fixture-verified |
| 14 | Submission                | 🟢 Passed     | Prepare → Commit → Confirm      | 10 outcome branches fixture-verified |
| 15 | One-trainee milestone     | 🟡 Blocked    | real end-to-end test            | **Gate 1** — awaiting operator |
| 16 | Initialize Rust           | 🟢 Passed     | `cargo check`                   |       |
| 17 | Rust models               | 🟢 Passed     | `cargo check`                   | `protocol.rs`, `report.rs` |
| 18 | Rust job                  | 🟢 Passed     | single-job flow                 | `job.example.json` |
| 19 | Rust queue                | 🟢 Passed     | `cargo test`                    | `queue.rs` |
| 20 | Rust state machine        | 🟢 Passed     | `cargo test`                    | `state.rs` |
| 21 | Rust ↔ Python protocol    | 🟢 Passed     | JSON request/response           | `docs/protocol.md` |
| 22 | Protocol validation       | 🟢 Passed     | invalid-message tests           | bad JSON / unknown op / job_id echo |
| 23 | Rust → Python connection  | 🟢 Passed     | IPC test                        | spawn + ready + ping |
| 24 | Real Rust → Python → DSSP | 🟡 Blocked    | one-trainee test                | **Gate 2** — awaiting operator |
| 25 | Rust coordinator          | 🟢 Passed     | `cargo test` (47)               | batch CLI + pre-flight dedupe; `2d2fcf9` |
| 26 | Retry policy              | 🟢 Passed     | retry tests                     | `decision.rs` + E2E backoff |
| 27 | Checkpointing             | ⬜ Not Started | checkpoint tests               | port TS `BatchCheckpoint.ts` |
| 28 | Recovery                  | ⬜ Not Started | crash/recovery tests            | **Gate 3** |
| 29 | Extension → Rust          | ⬜ Not Started | API integration                 | TS still at repo root |
| 30 | Pause                     | ⬜ Not Started | pause test                      | state exists; no engine API |
| 31 | Stop                      | ⬜ Not Started | stop test                       | abort path exists |
| 32 | Error testing             | ⬜ Not Started | failure scenarios               | many paths already covered |
| 33 | Full integration          | ⬜ Not Started | multi-layer test                | **Gate 4** |
| 34 | Remove old TS automation  | ⬜ Not Started | build + tests                   | **not before this stage** |
| 35 | Security review           | ⬜ Not Started | credential scan                 | design measures already in place |
| 36 | Final testing             | ⬜ Not Started | Rust + Python + extension       | **Gate 5** |
| 37 | Firefox                   | ⬜ Not Started | Firefox E2E                     | new scope |
| 38 | Documentation             | 🔵 In Progress | docs reviewed                   | README, protocol, rust README, runbook |
| 39 | Final verification        | ⬜ Not Started | full verification               | **Gate 6** |
| 40 | Migration complete        | ⬜ Not Started | Definition of Done              |       |

## Status Values

| Status         | Meaning                                                 |
| -------------- | ------------------------------------------------------- |
| ⬜ Not Started  | Stage has not been started                              |
| 🔵 In Progress | Agent is currently working on it                        |
| 🟢 Passed      | Stage completed and verification succeeded              |
| 🔴 Failed      | Stage encountered an unresolved problem                 |
| 🟡 Blocked     | Cannot continue because of an external/dependency issue |
| ⏸️ Skipped     | Explicitly skipped with a documented reason             |

### Status Rules

- **Not Started → In Progress** when work begins.
- **In Progress → Passed** only after the stage's verification succeeds.
- **In Progress → Failed** when implementation or verification fails and needs
  a fix.
- **In Progress → Blocked** only for an external dependency or missing
  requirement.
- **Failed / Blocked → In Progress** when fixing/resolving begins.
- Do not mark a stage `Passed` merely because the code compiles.

## Stage Completion Record

After every stage, append or update a record:

```text
Stage: 12 — Trainee Matching
Status: 🟢 Passed

Changes:
- Added name normalization.
- Added ID matching.
- Added ambiguous-name detection.

Verification:
- pytest
- 8 tests passed

Errors:
- None

Next:
Stage 13 — Training Form
```

For a failure:

```text
Stage: 14 — Submission
Status: 🔴 Failed

Changes:
- Added submission handler.

Verification:
- Submission reached DSSP.
- Confirmation failed.

Error:
SUBMISSION_INDETERMINATE

Action:
Do not retry submission.
Investigate confirmation logic.

Next:
Remain on Stage 14.
```

## Stage Records

```text
Stage: 25 — Rust Coordinator
Status: 🟢 Passed

Changes:
- Batch CLI: a job file with a "trainees" array runs BatchEngine over one
  portal session; a "trainee" key still runs the single-trainee path.
- Pre-flight resolution against list_trainees, then dedupe on the resolved
  canonical id — mirrors PortalClient._resolve_trainee, so the CLI and the
  worker agree on what a valid batch is.
- Direct decision.rs coverage (24 tests over decide_submit + settle_submit);
  11 more in main.rs over resolution and dedupe.
- rust/README.md documents the batch exit codes, incl. exit 4 meaning
  "already logged" as well as "rejected".

Verification:
- cargo test — 47 passed, 0 failed
  (24 decision, 11 resolution/dedupe, 6 engine, 4 state, 2 queue)
- commit 2d2fcf9

Errors:
- None

Next:
Stage 27 — Checkpointing
```

## Migration Progress Summary

```text
Completed:  23 / 40
In Progress: 1   (38)
Failed:      0
Blocked:     3   (10, 15, 24)
Skipped:     0
```

## Critical Gate Stages

Hard gates — do not continue past them until they pass.

- **Gate 1 — Stage 15:** Python → Playwright → DSSP → trainee → form →
  submission → confirmation.
- **Gate 2 — Stage 24:** Rust → Python → DSSP → Python → Rust.
- **Gate 3 — Stage 28:** crashes cannot cause unsafe duplicate submissions.
- **Gate 4 — Stage 33:** extension → Rust → Python → DSSP → Rust → extension.
- **Gate 5 — Stage 36:** all automated tests pass before cleanup.
- **Gate 6 — Stage 39:** full verification sequence succeeds.

## Current Execution Order

```text
10  Real DSSP session
 ↓
15  Gate 1 — one trainee
 ↓
24  Gate 2 — Rust → Python → DSSP
 ↓
25  Finish batch CLI
 ↓
27  Checkpointing
 ↓
28  Recovery
 ↓
29  Extension bridge
 ↓
30  Pause
 ↓
31  Stop
 ↓
32  Error testing
 ↓
33  Full integration
```

## Live-Run Procedure (Stages 10, 15, 24)

First run is headed so the operator can sign in once; the session persists in
`python/.pw-profile/` (gitignored). No credentials are ever stored.

**Stage 10 — session + retrieval (no writes):**

```bash
cd python
printf '{"v":1,"job_id":"live1","op":"ensure_session"}\n{"v":1,"job_id":"live2","op":"list_trainees"}\n' \
  | .venv/bin/python -m app.worker
```

Log in to DSSP in the Chromium window; expect `authenticated:true` and a real
trainee list.

**Stages 15 + 24 — one-trainee submit through the Rust chain:**

```bash
cp job.example.json job.json   # fill in a real trainee + instructor/type
cd rust
cargo run -- ../job.json
```

Expect `RESULT: confirmed …` (exit 0), `RESULT: duplicate …`, `FAILED: …`, or
`HALT: indeterminate …`. A single Rust-driven submit proves both gates; to prove
Gate 1 independently first, use a second trainee (a duplicate classification on
a re-run also proves the chain but must be verified on the portal).

## Current Execution Rule

```text
IF current stage = Passed      → move to next stage
IF current stage = Failed      → fix current stage
IF current stage = Blocked     → resolve blocker
IF current stage = In Progress → continue current stage
IF current stage = Not Started → begin current stage
```

Never skip a failed stage just to continue the migration.
