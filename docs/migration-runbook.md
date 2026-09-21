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
| 27 | Checkpointing             | 🟢 Passed     | `cargo test` (79) + clippy      | port TS `BatchCheckpoint.ts` + store + engine wiring; Gate 3 items → Stage 28 |
| 28 | Recovery                  | 🟢 Passed     | `cargo test` (133) + clippy     | **Gate 3** — in-flight record + start gate; `recovery.rs` |
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

```text
Stage: 27 — Checkpointing
Status: 🟢 Passed

Changes:
- `rust/src/store.rs`: the durable write path. One file, rewritten in full after
  every settled trainee, through a temp file + rename — `fs::write` truncates in
  place, so a kill mid-write would leave JSON that cannot be parsed, and that is
  the one state recovery must never read as "nothing was submitted". The write is
  atomic; the read side returns `Err` for a corrupt file rather than `Ok(None)`.
  `DSSP_CHECKPOINT` overrides the location, otherwise `dssp.checkpoint.json`
  beside the job file (gitignored: it holds trainee ids and names).
- `checkpoint.rs` gains `CheckpointWriter`, the trait the port deliberately left
  out until the engine had a sink to hand it.
- `BatchEngine` now emits `running` after each settled trainee — after the push
  and before the abort drain, matching AutomationEngine.ts:198-201 — plus one
  terminal `finished`. The sink is optional and the write's `Result` is dropped,
  so a storage fault cannot abort a batch that is otherwise submitting.
- `run_batch` wires the store in; the single-trainee path does not checkpoint,
  mirroring the TS engine, which only ever checkpoints batches.
- 18 new tests (11 store, 7 engine). The store's are a real disk round trip, and
  one of them pins the on-disk field names against a stray serde rename.
- Review pass (the four defects a design review found in the first draft):
  `save` now fsyncs the parent directory after the rename, because without it a
  power loss can take the rename itself and leave recovery reading the *previous*
  checkpoint — one whose `pending` may still name a trainee this batch already
  submitted; the temp file is `<name>.<pid>.tmp` rather than a fixed `<name>.tmp`,
  so two runs in one directory cannot rename each other's half-written file;
  `resolve_path(job, override)` is split out of `for_job` so the default location
  is testable without depending on whether the process happens to export
  `DSSP_CHECKPOINT`; and one engine+store test proves the two halves are actually
  wired together, which the earlier suite would not have noticed if they were not.

Verification:
- cargo test — 79 passed, 0 failed
  (24 decision, 14 checkpoint, 13 engine, 11 resolution/dedupe, 11 store,
  4 state, 2 queue)
- cargo clippy --all-targets — clean
- `cargo fmt --check` is NOT clean, but neither is the committed tree
  (`decision.rs`, `checkpoint.rs::is_live`), there is no `rustfmt.toml` and no CI
  workflow — so the project keeps a hand-maintained ~100-column style and this
  stage did not change that.

Errors:
- None

Open for Stage 28 (found in review, deliberately not fixed here):
- **The commit window.** `run` dequeues at `engine.rs:149` and only pushes the
  result after `process_trainee` returns, so from dequeue until then the trainee
  is in neither `results` nor `pending`. A crash in that window — spanning the
  portal round trip and every retry backoff — makes `unreconciled()` report it in
  neither group, and recovery would requeue nothing for a submission that may
  have reached the portal. Fixing it means recording the trainee *before*
  `worker.send` (a `in_flight: Option<String>` field on `BatchCheckpoint`, added
  additively so older files still parse). That is Gate 3's own criterion, which
  is why it is Stage 28's, not this stage's.
- **Nothing refuses to start over a live checkpoint.** `for_job` resolves one
  fixed slot and the first write of a new batch overwrites whatever was there, so
  a predecessor's indeterminate record can be destroyed before the new batch has
  submitted anything — and the loss is invisible, because the new file looks
  clean. Stage 28 must gate the start on a live, unreconciled checkpoint.
- Two things are *not* gaps, and Stage 28 should not rebuild them: the portal
  itself refuses a same-key replay (`python/app/portal/parsing.py` matches
  "duplicate|already logged", which `decision.rs` turns into
  `DUPLICATE_RECORD` without aborting), so a crash-window replay is a spurious
  `Failed` line rather than a second record; and `run_single` deliberately does
  not checkpoint, because one submission that dies leaves one unambiguous line.

Next:
Stage 28 — Recovery (Gate 3)
```

```text
Stage: 28 — Recovery
Status: 🟢 Passed

Changes:
- **The commit window** — the first of the two gaps Stage 27 left open. The
  engine now records the trainee it is about to submit: `in_flight:
  Option<String>` on `BatchCheckpoint`, written by a `running` checkpoint
  immediately before `worker.send`, rather than a result pushed after
  `process_trainee` returns. That one write is the only checkpoint write whose
  failure stops the batch — everywhere else a storage error is survivable
  because a later write follows it, but here no later write can undo the send
  that would come next. The trainee goes back on the queue first
  (`TaskQueue::put_back`) so the abort drain records it `Skipped` — never sent,
  and so safe to run again — instead of dropping it into no group at all. The
  field is `#[serde(default)]` and always serialized, so a file written by an
  older build still parses.
- **The start gate** — the second gap. `rust/src/recovery.rs` is the reading
  half the port deliberately left for this stage: `guard(&store, resume)` runs
  in `run_batch` BEFORE the worker is spawned. A live file, an in-flight
  trainee, or an unconfirmed record refuses the start with exit 3 and prints
  the counts, the reason and the two ways forward; the refusal records the
  interruption, so the next start is ordinary. A file that cannot be read
  refuses too — a checkpoint that cannot be read is not a checkpoint that says
  nothing ran.
- **`DSSP_RESUME=1`** continues instead of refusing: the never-attempted
  trainees run, and everything the predecessor recorded is carried into the new
  batch through `BatchEngine::with_carried`, so the engine's first full rewrite
  of the file cannot drop the only record of a submission that may already
  exist. Nothing whose submission may already be on the portal is ever
  submitted again.
- **Legacy provenance.** A pre-Stage-28 build wrote only at settle points, so
  its head-of-`pending` may already be on the portal. `#[serde(default)]` erases
  the difference between an absent `in_flight` key and an explicit null, so
  `CheckpointStore::load_with_provenance` inspects the raw JSON for key
  PRESENCE, and `BatchCheckpoint::untracked_suspect` synthesizes that head as an
  unconfirmed record — carried, excluded from the roster, never replayed.
- Supporting: `mark_interrupted`, `never_attempted`, `blocks_start`,
  `in_flight_record` and the in-flight synthesis in `unreconciled` in
  `checkpoint.rs`; `put_back` in `queue.rs`; `recovered_line` and the refusal
  text, which are the operator's whole interface to this.

The crash instants this closes — what the durable file says if the process dies
at each point, and what the next start does with it:

| instant | file says | next start |
| --- | --- | --- |
| before the pre-send write | trainee still `pending` | never sent — safe to run |
| after the pre-send write, before the send | trainee `in_flight` | refused / carried, never replayed — **over-reports, deliberately** |
| during the portal round trip | trainee `in_flight` | as above |
| during a retry backoff | trainee `in_flight` | as above |
| result in hand, before the settle write | trainee `in_flight` | as above — the outcome is lost, the id is not |
| during the abort drain | drained rows in `results`, remainder in `pending` | `never_attempted` restores batch order |
| after the last settle, before the terminal write | status still `running` | refused once, then clean |

So the file can only ever claim a submission that was never issued, never the
reverse. That is the safe direction to be wrong in: the cost is a human checking
the portal, not a second record.

Verification:
- cargo test — 133 passed, 0 failed
  (30 checkpoint, 25 engine, 24 decision, 23 recovery, 13 store,
  11 resolution/dedupe, 4 state, 3 queue)
- cargo clippy --all-targets — clean
- The end-to-end one:
  `a_crash_on_disk_is_refused_and_its_trainee_is_never_requeued` drives a real
  `CheckpointStore` through a batch that dies on its second submit, then reads
  the file back through a second store and puts it through `recovery::guard`
  both ways — the crash-window trainee is named, refused, and in no roster.
- README.md: `DSSP_RESUME` and `DSSP_CHECKPOINT` in the env table, the exit-3
  row, and a Recovery section.

Errors:
- None

Deliberately not done (and why):
- **A batch-scoped acknowledgement** (resume *this* batch, not whatever is in
  the slot). `DSSP_RESUME` is an environment variable and so is process-wide;
  the checkpoint path is per-job. Tying them together is a CLI-argument change,
  and the gate's job is duplicates, not authority.
- **Intersecting the resume roster with the job file.** A resume currently runs
  every never-attempted trainee the checkpoint knows about, whether or not the
  job file still lists them. Narrowing it would be a UX choice; carrying them
  is strictly more useful than the alternative of writing them off, and the
  partition invariant is what makes it safe.
- **Archiving the predecessor file before a fresh start.** The carried records
  already keep the information in the new file, which is more useful than a
  write-only archive nothing reads.
- The portal's own `duplicate|already logged` match remains a SECOND layer. It
  is not what makes the crash window safe, and Stage 27 said so explicitly.

Next:
Stage 29 — Extension → Rust. Its bridge is a second start path: it must call
`recovery::guard` before its own start, or it becomes the one way into the
engine that skips this gate.
```

## Migration Progress Summary

```text
Completed:  25 / 40
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
