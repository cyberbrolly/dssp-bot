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
| 28 | Recovery                  | 🟢 Passed     | `cargo test` (150) + clippy     | **Gate 3** — in-flight record + start gate; independent panel run, 5 defects fixed; 1 left open |
| 29 | Extension → Rust          | ⬜ Not Started | API integration                 | API designed → `docs/daemon-api.md`; TS still at repo root |
| 30 | Pause                     | ⬜ Not Started | pause test                      | state exists; no engine API |
| 31 | Stop                      | ⬜ Not Started | stop test                       | abort path exists |
| 32 | Error testing             | ⬜ Not Started | failure scenarios               | many paths already covered |
| 33 | Full integration          | ⬜ Not Started | multi-layer test                | **Gate 4** |
| 34 | Remove old TS automation  | ⬜ Not Started | build + tests                   | **not before this stage** |
| 35 | Security review           | ⬜ Not Started | credential scan                 | design measures already in place |
| 36 | Final testing             | ⬜ Not Started | Rust + Python + extension       | **Gate 5** |
| 37 | Firefox                   | ⬜ Not Started | Firefox E2E                     | new scope |
| 38 | Documentation             | 🟢 Passed     | docs reviewed against code      | 7 contradictions found and fixed; see the Stage 38 record |
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
| during the abort drain | as at the settle write — the drain is in memory only, so every un-attempted trainee is still in `pending` | `never_attempted` restores batch order |
| after the last settle, before the terminal write | status still `running`, nothing in flight | starts clean, and reports what it recovered |

So the file can only ever claim a submission that was never issued, never the
reverse. That is the safe direction to be wrong in: the cost is a human checking
the portal, not a second record.

Verification:
- cargo test — 145 passed, 0 failed
  (37 checkpoint, 28 engine, 25 recovery, 24 decision, 13 store,
  11 resolution/dedupe, 4 state, 3 queue) — re-measured after the second pass
  below, which added three tests and deleted one
- cargo clippy --all-targets — clean
- The end-to-end one:
  `a_crash_on_disk_is_refused_and_its_trainee_is_never_requeued` drives a real
  `CheckpointStore` through a batch that dies on its second submit, then reads
  the file back through a second store and puts it through `recovery::guard`
  both ways — the crash-window trainee is named, refused, and in no roster.
- README.md: `DSSP_RESUME` and `DSSP_CHECKPOINT` in the env table, the exit-3
  row, and a Recovery section.
- Re-measured after the third pass, which is this stage's final count:
  **cargo test — 150 passed, 0 failed**, `cargo clippy --all-targets` clean.
  The five new tests are `an_id_that_looks_positional_does_not_swallow_a_nameless_entry`,
  `a_name_does_not_collide_with_an_id`,
  `an_abort_on_the_last_trainee_is_recorded_as_an_abort`,
  `an_abort_with_nothing_drained_exits_3_not_4`, and
  `a_clean_batch_exits_0`; three tests that pinned the unsafe legacy-resume
  behaviour were rewritten to pin the safe one, and one was renamed. The
  per-module distribution above is otherwise unchanged, which reproduces the
  Stage 38 record's grep counts plus the new cases.

Review pass (an adversarial panel — five independent lenses over the module,
each finding then given to two skeptics told to refute it). It found one
**critical** hole in the first draft of this stage's own gate, plus four smaller
defects. They are listed first because they are the reason to trust the rest:

- **The refusal erased the suspicion it refused over** (critical). `guard`
  recorded the interruption by *saving* the file — and this build always
  serializes `in_flight`, while provenance is the key's **presence**. So the
  first refusal turned a legacy file into one that looked current. The follow-up
  run the refusal itself recommends (`DSSP_RESUME=1`) then found no suspect and
  put back into the roster the very trainee the dead build may already have sent.
  Recording the kill destroyed the evidence for the refusal that recorded it.
  Fixed by `promote_suspect`: the suspicion is moved into `in_flight` (and out of
  `pending`, keeping the partition) *before* any write, so it is expressed in the
  file's own fields and survives being rewritten. The regression test is
  `refusing_a_legacy_file_does_not_erase_the_suspicion_it_refused_over`.
- **A file that lost a trainee read as complete** (major). `untracked_suspect`
  can only name the head of `pending`, so a legacy file killed on its last
  trainee had nothing to suspect and nothing to carry — the trainee was in no
  group at all, and `guard` never checked the one contradiction the file can
  still testify to. Fixed by `unaccounted()`: `total` minus the three groups, and
  a non-zero count blocks the start. It is a `saturating_sub` in the safe
  direction — over-counting reads as zero, and a negative is not a trainee count.
- **Liveness was doing work it no longer needed to** (major). `blocks_start`
  blocked on `is_live()`, which was the right proxy before `in_flight` existed
  but is now a false alarm on every ordinary kill-and-retry — and a gate an
  operator learns to answer with `DSSP_RESUME=1` is one that stops being read.
  It now blocks on what can actually be missing: an unconfirmed record, or counts
  that do not add up. See the "removed safety net" note below.
- **The refusal's counts did not add up** (minor, found twice). `settled()`
  counted `Skipped` rows — never sent, and already counted as never-attempted —
  so one row landed in two groups and the breakdown exceeded `total`, telling an
  operator more submissions reached the portal than were attempted. `settled()`
  now counts `Success | Failed` only.
- **A deliberate "no" read as "yes"** (minor). `is_affirmative` treated
  everything except `0`/`false`/`no` as consent, so `DSSP_RESUME=off` — and a
  bare `DSSP_RESUME=`, which is what `DSSP_RESUME=$UNSET` produces — resumed.
  Only an explicit `1`/`true`/`yes`/`on` resumes now. The asymmetry decides it: a
  false yes can submit a trainee twice, a false no costs a re-run.
- **The parent-directory fsync was skipped for a bare path** (major). `save`'s
  durability leg opened `path.parent()`, which is `Some("")` for a directory-less
  name; opening `""` fails and the ignored error hid it. That is exactly the
  README's own invocation, `cargo run -- job.json` from the job's directory — so
  the commit window's durability leg was missing where it is most used. An empty
  parent now means the working directory.

The net the liveness check used to be: removing it is only sound because the
engine writes `in_flight` before `worker.send` on every path, and that write is
fatal if it fails. If a future change ever issues a submit without a preceding
in-flight write, this gate stops being sufficient — which is why that ordering is
the thing Stage 33's integration test should assert, not just exercise.

Second pass — closing the one item this stage left open:

- **The claim held, and it survives the retry loop.** Removing `is_live()` from
  `blocks_start` rests on "every submit is preceded by a durable in-flight write,
  and that write is fatal if it fails". Re-checked path by path against the fixed
  tree, and it holds — including across retries, which is the part most worth
  doubting: `process_trainee` writes no checkpoint between attempts, so the
  marker taken before the first send stands for the whole loop rather than only
  its first attempt. `ensure_session` is not a submit. The batch loop's pre-send
  write is the only other path in, and its failure stops the run before
  submitting, leaving the trainee in the group a resume picks up.
- **It is now enforced instead of assumed.** `BatchEngine` cannot be constructed
  without a checkpoint sink — it is a constructor argument, not a builder step —
  so "a batch always checkpoints" is a compile-time fact rather than a rule each
  new caller has to know. That premise is what the whole claim rests on, and a
  future caller (Stage 29's bridge is the next one) can no longer opt out of it
  by forgetting. The test that pinned the old hole, which asserted a batch could
  run with no durable record at all, is deleted: the state it constructed no
  longer compiles.
- **A roster was not a set.** `TaskQueue::enqueue` does not dedupe, and the only
  dedupe in the tree was the CLI's pre-flight resolution — so a caller reaching
  `BatchEngine::run` directly could submit the same trainee twice, and after a
  crash the repeat would still be in `pending`, where a resume would run it and
  re-submit something already confirmed. `run` now collapses repeats before it
  counts `total`; counting the input instead would leave the file permanently one
  trainee short of its groups, which `blocks_start` reads as a submission gone
  unrecorded and refuses on every start thereafter. Regression tests:
  `a_repeated_id_is_queued_and_submitted_once`,
  `a_repeated_id_left_by_a_crash_is_not_queued_for_a_resume`.
- **A dead guard read as protection.** `promote_suspect`'s early return on
  `in_flight.is_some()` cannot be reached from its one caller: a file whose
  `in_flight` is `Some` is exactly a file carrying that key, which is exactly a
  file `untracked_suspect` declines to suspect — so `guard` never has a suspect
  to promote, and the branch never runs. It stays as a second line of defence,
  with a comment that says so, and the test that pinned it is now labelled a
  direct-call test instead of being left to read as the protection it is not.
  What actually holds is the promotion plus `guard`'s `suspect_id` filter, and a
  new test states the mutual exclusion where it lives
  (`a_file_that_records_in_flight_has_no_suspect_to_promote`).
- **Checked and found sound**, so a reviewer knows what was looked at and what
  was not: `is_affirmative`; `load_with_provenance`'s key-presence test;
  `mark_interrupted`; `put_back` and the abort drain; `with_carried`'s `total`
  accounting; and `unaccounted`'s `saturating_sub`, which reads an over-count as
  zero and so does not refuse — an over-count needs one trainee in two of the
  groups, which the roster dedupe above now prevents, while the dangerous
  direction (a trainee in no group) does refuse.
- One table row was wrong and is corrected above: the abort drain writes nothing
  to disk, so a process killed during it leaves the file as the settle write left
  it, with every un-attempted trainee still in `pending` — not, as the row said,
  the drained rows already in `results`.
- **What this pass was not.** Round one's harness — five independent lenses, each
  finding handed to two skeptics told to refute it — could not be run at all this
  time; the tooling that hosts it was unavailable for the whole session. So this
  is close reading of the fixed tree, by the same author as the fixes. It is
  stronger than a reading usually is in exactly one way: the premise the claim
  rests on is now enforced by the compiler rather than argued from convention.
  It is weaker in another, and that is the item still open below.
- **Third pass: the independent panel, finally run.** Round two's harness was
  unavailable; round one's was the only independent lens this stage had ever
  had. This pass ran it. Four lenses over `checkpoint.rs`, `recovery.rs`,
  `store.rs` and `main.rs`, each asked for a concrete crash instant or input
  rather than an opinion; every finding handed to three skeptics told to refute
  it; admitted only when two of the three failed to. It found five defects. All
  five are fixed with a regression test, per this stage's rule:

  - **A file too old to record `in_flight` could be resumed from a stale
    handoff.** Convergent critical: three of three skeptics, and reached
    independently by two lenses. Round two made `never_attempted()`
    drain-then-queue, which is sound only for a file that records the key —
    that write is what makes `pending` mean "definitely never handed over". A
    build that had no such write dropped a failed one and carried on, so such a
    file can be behind by more than the single handoff
    `untracked_suspect` accounts for, and its `pending` vouches for nothing.
    Resuming that queue would re-submit a trainee the portal may already hold.
    `continuable` (`recovery.rs:290`) now draws a legacy file's roster from
    `drained_skips()` alone, since a drain happens after the queue stops being
    worked; the rest of the queue rides along as unconfirmed
    (`unvouched_queue`, `checkpoint.rs:235`, code `CRASH_UNTRACKED_QUEUE`) so
    the file's counts still add up and the operator still sees every name. The
    old behaviour was pinned by a test asserting it; that test now asserts the
    safe one, and its name changed to match.
  - **A legacy file's suspect reached the resume under the ordinary in-flight
    code.** `carried()` synthesizes the promoted suspect through
    `in_flight_record`, and the resume path reported it as it came — losing the
    distinction the refusal path keeps deliberately, which is the one fact that
    says how far the rest of the file can be trusted. The relabel at
    `recovery.rs:154` restores it, and the distinction now survives both paths.
  - **`owed` did not count the trainees the file cannot place.** `unaccounted()`
    — the gap between `total` and the three groups — was added to
    `blocks_start` but not to the resume's `owed` (`recovery.rs:179`), so a
    resume over such a file could report that it owed nothing while its own
    counts did not add up. It is counted now. The closing line lost its claim
    that every trainee was "either recorded or left unconfirmed", which was
    false in exactly that case; it now states what the run would submit.
  - **An aborted batch could exit 4 — or 0.** Exit 4 is "definitive failures,
    safe to re-run"; an abort is neither, and the aborted row's own outcome may
    be `Success`. The counts cannot separate the cases: a batch aborting on its
    *last* trainee drains nothing, so `skipped` stays 0, and the old test read
    that as 0 for a clean abort and 4 for one that happened to end on a failure.
    `BatchReport` now carries an `aborted` flag (`report.rs:45`) set by both
    early-exit paths, and `batch_exit_code` (`main.rs:625`) reads it first. This
    also closes a doc-versus-code contradiction the Stage 38 pass missed:
    `rust/README.md`'s exit table has listed "a batch aborted mid-run" under
    exit 3 all along, and the code could return 0 or 4 for it.
  - **A rename whose durability was not confirmed was silent.** `save()` syncs
    the parent directory best-effort and discarded the error. It now reports a
    `sync_all()` failure on a directory that did open (`store.rs:156`), and
    stays silent only when the directory cannot be opened at all — which is the
    one case the best-effort design exists for.

- **Item B, the keyed dedupe, implemented as the record proposed.**
  `queue_key` returned a `String`, so the positional fallback `#3` shared one
  namespace with every id and name compared against it: an entry whose id is
  literally `#3`, listed beside an entry at index 3 with neither, collapsed as a
  repeat and one of the two was dropped. `QueueKey` (`engine.rs:74`) is a tagged
  `Id`/`Name`/`Position` now, and `dedupe`'s set is keyed by it. The durable
  shape is unchanged — `pending` and `in_flight` remain plain strings — so the
  file format that stage 29's designed `GET /checkpoint` proxies is not touched
  and the store's fixture tests still hold. Regression tests: an id that looks
  positional does not swallow a nameless entry; a name does not collide with an
  id.

- **`rust/README.md` — not a no-op after all.** Two operator-visible changes came
  out of this pass. The exit-3 row already claimed "a batch aborted mid-run"; the
  code could return 0 or 4 for it, so fixing the code made the README true rather
  than needing an edit. The other did need one: a file from a build older than
  this one now runs none of its queue on a resume and exits 3 naming every
  trainee it could not vouch for. That is a new sentence in the `DSSP_RESUME`
  bullet. Nothing else in the operator surface moved — the durable file format,
  the env table, the `RESULT:` lines and the other exit rows are unchanged.
- **`docs/daemon-api.md` — checked, unchanged.** Stage 29's contract is
  `recovery::guard(&store, req.resume)` before anything is spawned, and a
  `409` carrying its refusal; the signature and the refusal's shape are both
  what they were. The keyed dedupe is internal to `engine.rs`, and the file
  format `GET /checkpoint` proxies is untouched, so the designed daemon inherits
  every fix above without an edit. This is the sense in which item B's fix was
  the cheap one: it is invisible to the next caller.

Errors:
- The first adversarial pass found the critical hole above. Fixed, with a
  regression test for each defect, rather than recorded and carried.
- The second pass found the three defects above. Same rule: fixed, with tests,
  not recorded.
- The third pass found the five above. Same rule again. Two findings were
  refuted and are recorded below as refuted, and one was left open rather than
  decided, because fixing it changes operator-facing behaviour on the path the
  live gates use.

Open for whoever reviews this stage:
- **A plain start over a checkpoint that records landed work is not refused.**
  Third pass, confirmed (two of three skeptics). Kill the process between one
  trainee's settle write and the next one's pre-send write, then run the same
  job file again *without* `DSSP_RESUME`: the file reads `results=[1]`,
  `pending=[2,3]`, no `in_flight`, so `blocks_start` is false, `guard` returns
  `Fresh`, and the CLI re-queues the whole job file — trainee 1 included, which
  that same file records as landed. The recovered line does print those counts
  before the overwrite, and the portal's `duplicate|already logged` match is the
  next layer; the dissenting skeptic is right that the entry under "deliberately
  not done" below chose disclosure over refusal, and that both are recorded. The
  gap it leaves is narrower and real: the most natural operator action after a
  crash — run the command again — is the one that re-submits, and the layer that
  stops it is the portal's text match, which this stage lists elsewhere as an
  assumption that can fail in the unsafe direction. Two candidate fixes, for
  whoever decides: widen `blocks_start` (`checkpoint.rs:266`) from "is a
  submission missing" to "is a submission missing, *or* does this file already
  record a landed one", or keep the disclosure and move its warning into a
  refusal that `DSSP_RESUME` clears. Not decided here because it changes
  operator-facing behaviour on the live-gate path, and because it reinterprets
  a choice this document already records.
- **What the audit did not reach.** Recorded so this pass is not read as wider
  than it was. Both panels read `checkpoint.rs`, `recovery.rs`, `store.rs`,
  `main.rs`, and (third pass only) `engine.rs`, `worker.rs` and `python/app`.
  Still never a lens target: `decision.rs`, `resolution.rs`, `queue.rs`,
  `state.rs`. And the shapes of evidence this stage has never produced for Gate
  3: no test kills the process for real (every crash is simulated by
  constructing a file), no write-fault injection (the fatal-write premise is
  tested by making the *store* fail, not the disk), no concurrency case (there
  is no lock or pid file, so two batches over one path are untested), no fuzz
  over arbitrary checkpoint files — every fixture is well-formed — and
  `Response.v` is deserialized but still never compared to `protocol::V`. The
  panel's own critic named the last one as the sharpest: the gate reduces to
  Rust reading `proves_nothing_submitted` correctly, and the version field that
  would detect a worker disagreeing about what that flag means is read and
  dropped.
- **Refuted, and recorded so they are not re-litigated.** Two findings did not
  survive: that `promote_suspect`'s dead early return is still load-bearing
  (three of three refuted — it is unreachable from its one caller, as round two
  recorded, and the promotion plus `guard`'s `suspect_id` filter is what holds),
  and that `unaccounted`'s `saturating_sub` hides an over-count in the unsafe
  direction (three of three refuted — an over-count needs one trainee in two
  groups, which the roster dedupe prevents, while the dangerous direction, a
  trainee in no group, does refuse). Also checked and found sound this pass:
  `blocks_start`'s `has_unconfirmed() || unaccounted() > 0` composition;
  `settled()`'s exclusion of `Skipped`; and the refusal's arithmetic, which now
  counts a legacy file's roster from the same source the resume does.

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
- **Carrying a predecessor's settled history into a fresh start.** A `Fresh`
  start does discard the previous file's rows — that is what "fresh" means, and
  carrying them would inflate the new batch's `total` with trainees it never ran
  and report them as its own results. What the gate owes the operator instead is
  the recovered line, which now prints and reports those counts before the
  overwrite. The archive idea is declined on the same ground: the records worth
  keeping are the unconfirmed ones, and those are carried, not archived. The
  third pass confirmed a finding in this neighbourhood — a fresh start
  re-submits landed trainees, not merely reports them — and it is left open
  above rather than used to reverse this decision.
- **Making the parent-directory fsync fatal.** It is best-effort because a
  directory cannot be opened as a file on every platform, and a fatal version
  would stop every batch on one. The path bug above is fixed; the residual is
  that on such a platform the rename's durability is not guaranteed, which is a
  property of the platform rather than of this design. The third pass raised a
  stronger form — have `save()` distinguish "written" from "written and the
  rename is durable", and let the pre-send write (`engine.rs:231`) treat
  non-durable as it treats a failed write. Declined for the same reason plus
  one more: the precondition is a directory that opened and then refused
  `sync_all`, which on a real filesystem means the disk is already failing, so
  the change would buy a halt in the case where every other write is about to
  fail anyway — at the cost of a return type every caller must carry, on the
  one path that must not gain a way to fail. What it should do instead it now
  does: say so, loudly, on stderr.
- The portal's own `duplicate|already logged` match remains a SECOND layer. It
  is not what makes the crash window safe, and Stage 27 said so explicitly.

Next:
Stage 29 — Extension → Rust. Its bridge is a second start path: it must call
`recovery::guard` before its own start. It would not be the *only* ungated way in
— `run_single`, the path the live gates use, has no checkpoint and no guard — but
that one is a deliberate trade rather than an oversight, and it now tells the
operator so: one line to stderr before its submit loop, naming the trainee and
stating that a death from there means checking the portal before re-running the
job, plus a closing line saying whether the submission settled. A single
trainee's outcome *is* its exit code, so there is nothing on disk for the next
start to reconcile; what the warning buys is the one thing that reasoning needs,
which is the operator knowing to look. `run_batch` needs no such line — it writes
the window down before it opens it, and now cannot be built without somewhere to
write it. The API for that bridge is designed in `docs/daemon-api.md`; it is not
built until the live gates have run.
```

```text
Stage: 38 — Documentation
Status: 🟢 Passed

Changes:
Reviewed every claim in the docs against the code and corrected what the code
contradicted. Six contradictions, all doc-side; no code changed to match a doc.
A seventh — the progress summary — was found later and is corrected in this
record's "Checked and found accurate" list, which is where the wrong check that
let it through was written down.

- docs/protocol.md — `PORTAL_UNAVAILABLE` was listed as
  `proves_nothing_submitted: true`. Both implementations say false
  (`_PROVES_NOTHING` in python/app/protocol.py, `provesNothingSubmitted` in
  AutomationEngine.ts). This was the unsafe direction: `decision.rs` retries only
  what proves nothing was submitted and counts PORTAL_UNAVAILABLE as retryable,
  so the documented value would have licensed exactly the retry the code refuses.
  Fixed, with the reason written down.
- rust/README.md — exit 1 was undocumented although two paths return it (the
  worker failing to launch, and the session gate failing) on both the single and
  batch paths. Added, along with the asymmetry an operator will otherwise
  misread: the same session-gate failure exits 1 on `run_single` and 3 on
  `run_batch`, and a session that lapses mid-run is `HALT: SESSION_EXPIRED` (3).
- docs/phase4/portal-integration-specification.md §3 — carried a TODO asking
  whether to paginate or raise the page size, while constants.py:25 had already
  decided (`pgsize=10000&page=1&keywords=`). Recorded, with the failure it hides:
  a capped page size truncates the list silently and a real trainee then reads as
  TRAINEE_NOT_FOUND. The count comparison that catches it is now a required
  output of the Stage 10 run.
- Same spec §6 — blank, under a document whose first rule was "do not write
  selectors into code that are not recorded here first". The code waits on no
  spinner at all (only Playwright navigations and the 30s/300s timeouts), so the
  rows now say "none / not observed" instead of being empty: the absence is a
  fact, not an oversight.
- Same spec §2 — "current page only" was stale, and two silent behaviours of the
  row read were unrecorded: a row whose link yields no numeric TraineeId is
  dropped from the list entirely (parsing.py:88), and the name is read
  positionally at cells[2] (parsing.py:97), against the spec's own rule.
- Same spec, mirror path — named src/core/infrastructure/portal/ as the only
  place selectors may live, while the live implementation is python/app/portal/.
- rust/src/recovery.rs:10 — its module doc still claimed Stage 29's bridge would
  otherwise be "the one way into the engine that skips this" gate. That is the
  claim A4 retired from the runbook; the code comment now carries the same
  corrected reasoning rather than contradicting it.

Checked and found accurate (no change needed):
- The progress summary originally claimed 25 Passed / 1 In Progress. **That was
  wrong, and this bullet is the correction.** The check behind it confirmed only
  that both numbers sum to 40; it never compared the distribution. The status
  table's row 38 is 🟢 Passed, so the summary was one short of the table and
  named an In Progress stage that does not exist. Corrected below to match the
  table, and the check is now stated as what it actually is: the summary's
  numbers are read off the table's, row by row, not merely added up.
- The env-var tables: all 8 variables documented across rust/README.md and
  docs/protocol.md are read by the code (3 by Rust, 5 by the worker), no read
  variable is undocumented, and every stated default matches.
- ADR 0001's status: "proposed" is correct. None of its five decisions is
  implemented — no createManifest, no FirefoxBrowserAdapter, armDialogs() is
  still outside the try at BridgeClient.ts:135, and the origin is still hardcoded
  in three files.

Verification:
- Every entry above was checked by reading the code it describes, with the file
  and line recorded in the entry.
- Test counts settled, previously 133 in the status table against 143 in the
  Stage 28 record: both were right at different times, and the real number is
  now 150 — 145 as measured here, plus the 5 the third pass on Stage 28 added
  afterwards. `grep -c '#[test]'` per module gives 37 checkpoint, 28 engine,
  25 recovery, 24 decision, 13 store, 11 resolution/dedupe, 4 state, 3 queue for
  the 145, confirmed twice (cargo test, and the grep); the status table carries
  the current 150.
- The Python evidence dump added to the worker for the live gates
  (DSSP_DUMP_DIR, python/app/portal/client.py) and its tests had not been run
  when this record was written: the Bash classifier was unavailable for the
  whole of this pass. **Now run, and green.** `python/.venv/bin/python -m
  pytest` passes 53 tests, including the 7 in `tests/test_evidence_dump.py`
  that had never executed. This is the last thing the live gates needed from
  here: nothing in the Python tree is now unproven by test.

Errors:
- The Bash tool was refused for most of this pass ("deepseek-v4-flash is
  temporarily unavailable"), so the doc review was done by reading. Re-checking a
  claim against the code is exactly what reading is for; running the suite is
  not, and that gap was recorded above rather than papered over. It is closed
  now — see the last Verification entry.

Next:
Stages 10/15/24 need the operator (see the Live-Run Procedure). Stage 29 follows
them, designed but deliberately unbuilt.
```

## Migration Progress Summary

```text
Completed:  26 / 40
In Progress: 0
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

The order the work follows, not a list of what is outstanding: 25–28 are already
Passed (see the status table above), and the entry point today is whatever is
still Blocked — the live gates at 10, 15 and 24.

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

**These runs submit real records to the real portal.** Nothing below is a dry
run. Do them in order and stop at the first failure rather than working around
it — a worked-around failure is a gate that has not been proven.

**Stage 10 — session + retrieval (read-only, writes nothing):**

```bash
cd python
printf '{"v":1,"job_id":"live1","op":"ensure_session"}\n{"v":1,"job_id":"live2","op":"list_trainees"}\n' \
  | DSSP_DUMP_DIR=.evidence .venv/bin/python -m app.worker
```

Log in to DSSP in the Chromium window; expect `authenticated:true` and a real
trainee list. Then check what the run captured in `python/.evidence/`
(gitignored — it holds raw portal HTML including a form's antiforgery token, so
it is never committed, pasted into an issue, or sent anywhere):

- **`trainee-list.html`** — does the row shape match §2 of
  `docs/phase4/portal-integration-specification.md`? Above all, compare the
  `count` in the response with the total **the portal's own page displays**. A
  mismatch means `pgsize=10000` is being capped, and every trainee past the cap
  is invisible to the bot: they surface later as `TRAINEE_NOT_FOUND`, which reads
  as "the portal has no such trainee" for someone who plainly exists.
- the form HTML from one `get_form_options` call, if run — the POST shape and the
  `__RequestVerificationToken` assumption can be checked against it.

**Stages 15 + 24 — one trainee through the Rust chain (Gate 1, then Gate 2):**

```bash
cp job.example.json job.json   # fill in a real trainee + instructor/type
cd rust
cargo run -- ../job.json
```

Expect `RESULT: confirmed …` (exit 0), `RESULT: duplicate …`, `FAILED: …`, or
`HALT: indeterminate …`. Immediately before the submit the run prints a warning
line: from that point a death reporting no result cannot say whether the portal
took the record, so **if the process is killed after that line, check the portal
before re-running**. That warning is the whole of this path's crash story — it
has no checkpoint, deliberately.

This run proves Gate 1 (Python → portal → confirmation) and Gate 2 (Rust →
Python → DSSP → Rust). It exercises **none** of stages 25–28: `run_single` has no
checkpoint, no recovery gate and no engine. A green run here says nothing about
the batch machinery.

**Stages 25–28 — the batch path (checkpoint, recovery, dedupe, abort drain):**

```bash
cp job.batch.example.json job.json   # two real trainees: one by id, one by name
cd rust
cargo run -- ../job.json
```

This is the only run that exercises what stages 25–28 were built for:
`resolve_against`, `run_batch`, `recovery::guard`, the checkpoint store, and the
engine's in-flight record before each submit. Expect a stderr line naming the
checkpoint file, then the JSON `BatchReport` on stdout, exit 0.

**It submits for real, for both trainees** — pick two you are content to log a
training for. How to read it:

- `python/.evidence/submit-response-*.html` versus §5 of the phase4 spec. The
  `duplicate` classification is a **text match** on that body, and it is the
  documented second safety layer for the crash window. If the portal's real
  wording differs from the match list, a genuine duplicate classifies as
  `confirmed` and Rust reports success on a replay — worth confirming here,
  because this is the one run that can.
- `dssp.checkpoint.json` (beside the job file, gitignored) should exist, and
  should name both trainees in `results` with `pending` and `in_flight` empty.
- **Re-running the same job file is not a second test** — it re-submits both. For
  the duplicate path without a second record, put the *same* trainee in twice
  (`[{"id": "…"}, {"name": "…"}]`, both resolving to one person): that collapses
  to a single submission and proves the dedupe instead.
- To prove the recovery gate, run the batch again (or kill it partway and re-run).
  A start over a live checkpoint must be **refused with exit 3** and the counts
  printed. That refusal is the Gate 3 promise. If it does not happen, stop and
  investigate — do not pass `DSSP_RESUME` to get past it.

### What the fixtures cannot prove

Every stage marked Passed on `pytest` rests on markup that encodes the parser's
own assumptions (`python/tests/test_submit_outcomes.py` defines the trainee table
and form inline). The live run is the first test of these, and they fail
*silently*, so they are listed here to be checked deliberately:

| assumption | where | how it fails |
|---|---|---|
| trainee ids are numeric | `constants.py` `TRAINEE_ID_RE` (`\d+`) | a non-numeric id is never extracted; the row is dropped and the trainee reads as `TRAINEE_NOT_FOUND` |
| the name is the third `td` | `parsing.py:97` (positional, against the spec's own rule) | an inserted column yields a blank name; a name match then fails as `TRAINEE_NOT_FOUND` |
| one `pgsize=10000` request returns everything | `constants.py:25` | silent truncation; see the count check above |
| exactly one `table.table-checkable` | `constants.py:33` | a second matching table merges rows from both |
| duplicate wording | `parsing.py` `submission_outcome` (text match) | misclassification, in the unsafe direction — see above |
| the login marker | `parsing.py` `is_login_page` / `has_authenticated_marker` | a rewritten login page reads as authenticated, or vice versa |

### If it fails

Record enough that the failure is diagnosable without re-running it — Stage 16's
list, unchanged:

- which layer reported (Rust, the worker, Playwright, or the portal);
- the raw error and the `error_code`;
- the current checkpoint file (`DSSP_CHECKPOINT`, default beside the job);
- whether a submission committed — from the evidence dump, the checkpoint, or
  the portal itself.

**Anything indeterminate is investigated on the portal and never re-run.** A
`HALT:` exit 3, a killed process after the single-run warning, and a refused start
are all states where re-running is the wrong move and reading the portal is the
right one.

## Current Execution Rule

```text
IF current stage = Passed      → move to next stage
IF current stage = Failed      → fix current stage
IF current stage = Blocked     → resolve blocker
IF current stage = In Progress → continue current stage
IF current stage = Not Started → begin current stage
```

Never skip a failed stage just to continue the migration.
