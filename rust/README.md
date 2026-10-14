# DSSP-Bot Core (Rust coordinator)

Coordinator: spawns the Python Playwright worker and owns every retry/stop
decision. Drives one trainee end to end (the live gates), or a whole batch
through a single session.

```
Rust (this crate) → Python worker (stdio JSON) → Playwright → DSSP → result
```

Protocol: see [`../docs/protocol.md`](../docs/protocol.md).

## Prerequisites

- Rust toolchain (built with 1.98; `edition = "2024"`).
- The Python worker set up once:

  ```bash
  cd ../python
  python3 -m venv .venv
  .venv/bin/pip install playwright beautifulsoup4 lxml
  .venv/bin/playwright install chromium
  ```

The coordinator runs `../python/.venv/bin/python -m app.worker` by default
(override with `DSSP_WORKER_PY`).

## Run one trainee

```bash
cp ../job.example.json job.json     # gitignored; edit with a real trainee + details
cargo run -- job.json               # (defaults to ./job.json if no arg given)
```

`job.json` shape:

```json
{
  "trainee": { "id": "12345" },
  "session": { "training_date": "2026-09-14", "instructor": "Jane Smith", "training_type": "Practical" }
}
```

- Use `"trainee": { "name": "John Doe" }` instead of `id` to match by name —
  but an **ambiguous name stops the job** (supply the id instead).
- `instructor` / `training_type` accept the portal value **or** its label.
- `training_date` accepts `YYYY-MM-DD` or `DD/MM/YYYY`.

### First run: log in once

The first run opens a real Chromium window and **waits for you to sign in** to
DSSP manually. The session is stored in `../python/.pw-profile/` (gitignored),
so later runs reuse it. Run headless once logged in with `DSSP_HEADLESS=1`.

## Run a batch

Give the job file a `trainees` array instead of a single `trainee`:

```json
{
  "trainees": [{ "id": "12345" }, { "name": "Jane Roe" }],
  "session": {
    "training_date": "2026-09-14",
    "instructor": "Jane Smith",
    "training_type": "Practical"
  }
}
```

```bash
cargo run -- job.json
```

Every entry is resolved against the portal's trainee list **before anything is
submitted**: an explicit `id` must exist, a `name` must match exactly one
trainee, and an ambiguous name stops the whole batch with the offending entries
listed. Entries resolving to the same trainee collapse into one submission.
Progress goes to stderr; the JSON `BatchReport` goes to stdout.

| exit | meaning |
| ---: | ------- |
| 0 | every trainee recorded |
| 1 | the run never got as far as submitting: the worker would not launch, or the session gate failed |
| 2 | the batch could not be resolved — nothing was submitted |
| 3 | needs a human: a start refused over an unfinished batch, an indeterminate submission, or a batch aborted mid-run |
| 4 | definitive failures, safe to re-run |

Exit 1 is the coordinator failing to get going, and it prints no `RESULT:` line —
only a `dssp-bot:` line on stderr. In a *batch* the same session-gate failure
exits 3 rather than 1 (the batch path treats a lapsed session as something a
human has to clear, not as a startup failure). Either way nothing was submitted.

In a batch, exit 4 also covers **already-logged** trainees: a `duplicate` is
recorded against that row as a failure, so exit 4 means "something needed
attention", not necessarily "the portal refused a record". A single-trainee run
still reports a duplicate as `RESULT: duplicate` and exits 0.

## Recovery

A batch keeps its state in a checkpoint file (see `DSSP_CHECKPOINT`). It is
written before each submission, not only after: the file **names the trainee
being submitted before the submit is issued**, so a file that does not name a
trainee is a file for which no submit was sent. That write is fatal if it fails —
the batch stops *before* submitting rather than submitting unrecorded.

So a crash can only ever leave the file claiming a submission that was never
issued, never the reverse. That is the safe direction to be wrong in: the cost
is a human checking the portal, not a second record.

The next start reads that file first, before it connects to anything:

- **May be missing a submission** — a submission that was in flight, a record
  that was never confirmed, or counts that do not add up. The start is refused
  with the counts and the ways forward (exit 3), and the file is marked
  interrupted. The refusal stands until it is acknowledged — recording the
  interruption does not clear it, and neither does running the command again.
- **Already landed work** — a batch that stopped early holding records that
  reached the portal, whether it was killed or aborted itself. A plain start
  re-queues the job file from the top, so it would ask the portal for those
  trainees a second time; refused (exit 3), rather than left to the portal's own
  duplicate match. This is narrower than "being killed": a kill *before* anything
  landed still starts clean, and a batch that worked through its whole queue is
  an ordinary re-run, as is a re-run of a completed job file. An aborted batch
  writes `aborted`, not `finished`, precisely so it is not mistaken for one that
  closed out.
- **`DSSP_RESUME=1`** — continue it instead. The never-attempted trainees run;
  everything the predecessor recorded is carried forward into the new file so it
  stays whole. Nothing whose submission may already be on the portal is ever
  submitted again — those are reported and left for you. If there is nothing left
  to attempt, the run exits 0 when every loose end is settled and 3 when a human
  is still owed; in the exit-0 case it also writes the batch closed out, so the
  file stops refusing later starts. The acknowledgement is what lifts the
  refusal, not a repeat of the original command.
  A file from a build **older than this one** — written before the checkpoint
  named a submission in flight — runs none of its queue: it cannot say how far
  through it got, so every trainee still waiting in it is carried forward as
  unconfirmed and the run exits 3 with the names. Check the portal for them.
- **Unreadable** — a corrupt file is not a file that says nothing ran. Refused,
  never treated as an empty slot.

A checkpoint written by a build older than this stage has no record of what was
in flight (older builds only wrote after a submission settled), so its first
unattempted trainee is *presumed* to have reached the portal and is left for you
rather than resumed.

## Configuration (environment)

| var | default | purpose |
|---|---|---|
| `DSSP_ORIGIN` | `https://dssp.frsc.gov.ng` | portal origin |
| `DSSP_HEADLESS` | `0` | `1` = run Chromium headless |
| `DSSP_PROFILE_DIR` | `../python/.pw-profile` | persistent login profile |
| `DSSP_LOGIN_TIMEOUT_MS` | `300000` | how long to wait for manual login |
| `DSSP_NAV_TIMEOUT_MS` | `30000` | per-request timeout |
| `DSSP_WORKER_PY` | `../python/.venv/bin/python` | worker interpreter |
| `DSSP_CHECKPOINT` | `dssp.checkpoint.json` beside the job | where batch state is written |
| `DSSP_RESUME` | unset | `1`/`true`/`yes`/`on` = continue an interrupted batch; anything else means no |

## Result & exit codes

The coordinator prints one line to stdout (progress/logs go to stderr):

| line | exit | meaning |
|---|---|---|
| `RESULT: confirmed …` / `RESULT: duplicate …` | 0 | recorded (or already present) |
| `HALT: indeterminate …` | 3 | **submitted but unconfirmed — verify on the portal, do not re-run blindly** |
| `HALT: SESSION_EXPIRED …` | 3 | session lapsed / portal changed — stop |
| `FAILED: rejected …` / `FAILED: TRAINEE_NOT_FOUND …` | 4 | definitive failure, no retry |

No row here exits 1: that code means the coordinator never reached a submission
(worker would not launch, or the session gate failed before the submit loop), so
there is no classified line to print. A session that lapses *mid-run* is a
`HALT: SESSION_EXPIRED` and exits 3, as above.

## Stages 29–31: loopback daemon

The CLI above remains unchanged. `cargo run --offline -- --daemon` selects the
HTTP bridge; the extension never starts or owns this process. Starting a daemon
alone does not start a worker. Authenticated `/form-options` reads and admitted
`/batch` requests can start the configured Python worker and contact the portal;
**do not use production requests as tests**.

Example configuration (replace the extension origin with the installed ID):

```sh
DSSP_DAEMON_ORIGINS=chrome-extension://your-extension-id cargo run --offline -- --daemon
```

| variable | default | daemon meaning |
|---|---|---|
| `DSSP_DAEMON_PORT` | `8765` | TCP port, `1..65535`; bind is always `127.0.0.1` |
| `DSSP_DAEMON_ORIGINS` | required | comma-separated **exact** `chrome-extension://id` / `moz-extension://uuid` origins; no whitespace, paths or wildcards |
| `DSSP_PROFILE_DIR` | `.pw-profile` relative to `python/` | same profile location the worker uses; token is `dssp.daemon.token` beside this directory |
| `DSSP_CHECKPOINT` | `./dssp.checkpoint.json` | existing checkpoint slot; relative paths use the daemon's working directory |
| `DSSP_WORKER_PY` | existing worker default | unchanged interpreter override |
| `DSSP_RESUME` | ignored | startup logs that it is ignored if present; only HTTP `resume` acknowledges recovery |

The profile's parent directory must already exist. On first startup the Linux
daemon obtains **32 bytes / 256 bits** from `/dev/urandom`, hex-encodes them,
and creates an owned regular token file with mode `0600`. Later starts reuse it.
Unsafe permissions, symlinks, hardlinks and malformed token files fail closed.
The token value is never logged or returned over HTTP. Manually paste the file's
contents into extension storage; there is no discovery endpoint. This
implementation's token checks use Linux `/proc/self` and Linux open flags;
non-Linux daemon portability is not established.

### Extension interface

Base URL: `http://127.0.0.1:8765` (or the configured port). Every request requires
`Authorization: Bearer <token>`. An absent `Origin` is accepted with a valid
token; a supplied origin must exactly match the configured allowlist. Responses
are **direct JSON**, not `{success,data}` messaging envelopes. No CORS allow
headers are emitted and `OPTIONS` is unsupported.

| method/path | response |
|---|---|
| `GET /status` | `200` + `BatchProgress` |
| `GET /report` | `200` + latest in-memory report, or `null` |
| `GET /checkpoint` | `200` + durable checkpoint, or `null`; corrupt storage is `500` |
| `GET /form-options` | `200` + `{instructors: [{value,label}], trainingTypes: [{value,label}]}` |
| `POST /batch` | `202` + `{batchId}`; recovery refusal `409`; invalid/unresolvable input `422`; active worker `503` |
| `POST /batch/pause`, `/batch/resume` | `200` + raw `BatchProgress`; no controllable batch `409` |
| `POST /batch/stop` | `200` + raw `BatchProgress` with latched stop intent; no controllable batch `409` |

Unknown methods/paths return `405`. Authentication failures are `401`, foreign
origins `403`, non-JSON batch/control content types `415`, malformed HTTP `400`, excessive
headers/bodies `413`, and request read timeout `408`. Worker startup/session/list
failures return `502`. All errors have shape `{error: string}`. Admission resolves
the whole roster before returning `202`; no submissions happen on a `422` path.
A `202` is not submission confirmation; poll status and inspect the report.

`POST /batch` requires `Content-Type: application/json` and this body:

```json
{
  "trainees": [{"id": "123"}, {"name": "Jane Roe"}],
  "session": {
    "training_date": "2026-09-14",
    "instructor": "Jane Smith",
    "training_type": "Practical"
  },
  "resume": false
}
```

`resume` must be a JSON boolean (omitted means `false`). Explicit `true` uses the
predecessor's never-attempted roster, not the requested trainees; the operator
must supply the original session template because the checkpoint does not store
it. `recovery::guard` runs before any worker creation, including handshake.
Settled/possibly submitted trainees are carried, never requeued.

`/status` uses the exact TS `BatchProgress` names: `state`, `total`, `processed`,
`successful`, `failed`, `skipped`, `indeterminate`, `remaining`, and optional
`currentTraineeName`, plus `pauseRequested: boolean` and `stopRequested: boolean`. Checkpoint writes drive progress during execution; the
worker's internal browser/form phases are not separately exposed. Report and
checkpoint keys are camelCase on HTTP and unchanged snake_case on disk/CLI.
Timestamps remain **epoch-millisecond strings**, not the legacy TS ISO format.
HTTP checkpoints additionally include `inFlight` and may have status `aborted`;
reports include `aborted`. Consumers must preserve those recovery semantics.
`batchId` is an admission identifier, not a durable checkpoint field.

The batch runs on its own thread; status stays available during worker admission
and execution. Another batch or form-options worker is refused with `503`.
HTTP/1.1 parsing rejects duplicate headers, transfer encoding (including chunked),
`Expect`, folded/control-character headers and ambiguous lengths. Request bodies
are capped at 64 KiB, headers at 16 KiB, with a two-second total read deadline.
Connections close after one response; pipelining is not supported. At most 16
connection handlers run at once; excess connections are closed.

Only the checkpoint survives daemon restart: `/status` begins idle and `/report`
returns null, while `/checkpoint` still exposes the durable predecessor and the
start guard protects it. Do not run a CLI batch and daemon, or multiple daemons,
against the same profile/checkpoint concurrently: there is no cross-process
ownership lock. Graceful daemon shutdown remains out of scope.

### Stage 30: cooperative pause/resume

`POST /batch/pause` and `POST /batch/resume` accept only an empty JSON object
`{}` (whitespace is allowed) with `Content-Type: application/json`. They share
batch authentication, Origin validation, body/header caps and read deadlines.
Invalid control bodies return `422`. Successful controls return `200` with raw
progress, not an envelope or a new batch ID.

- **Pause is intent, not cancellation.** While a trainee is being submitted or
  settled (including its existing safe retry policy), pause returns the current
  active state with `pauseRequested: true`. The in-flight request is never
  interrupted, requeued or duplicated. An unknown/indeterminate result still
  aborts with no retry; abort and exhausted-queue completion take precedence over
  pause.
- **The safe boundary is before dequeue.** With pending work, the engine persists
  `status: paused`, all settled results, the unchanged pending queue, and
  `in_flight: null` before publishing progress `state: paused`. It then waits on
  a condition variable, retaining the same worker, session, queue and batch.
  `/status` remains responsive; another batch or form-options request is `503`.
- **Resume is in-memory intent.** Resume clears `pauseRequested` and wakes the
  existing engine; it does not create a worker or reconstruct/requeue the batch.
  The response can still be `state: paused` while the engine wakes. The engine
  must persist `status: running` at this safe boundary before any later submit.
  Failure of either pause or resume checkpoint halts execution with no later
  POST and clears the controller; it never publishes a failed paused write.
- **Idempotence is scoped to an existing controller.** Repeated pause while
  pending/paused and repeated resume while active return `200`. Resume during a
  pending pause cancels that intent. Idle, finished/aborted batches, worker
  teardown, form-options, and batch admission before the controller is installed
  return `409` without changing progress or creating a worker. Admission finishes
  session/roster resolution before installing the controller; retrying a control
  after an accepted `202` is an explicit caller request, never automatic recovery.
  Errors/panics clear controls before worker teardown; admission stays busy until
  shutdown finishes. A new batch always gets fresh, unpaused control state.
- **Restart recovery is a different operation.** A persisted paused checkpoint
  does not create an in-memory controller after daemon restart. `/batch/resume`
  returns `409`; plain `POST /batch` is refused by the existing recovery guard
  before creating any worker. Only an explicit `POST /batch` with `resume: true`
  acknowledges checkpoint recovery, carrying settled results and attempting only
  never-submitted pending trainees. The original session template is still needed.

Normal CLI execution has no controller and is unchanged. These are synthetic,
offline semantics, not a claim of live batch acceptance.

### Stage 31: cooperative, latched stop

`POST /batch/stop` requires the same bearer authentication, allowed Origin,
`Content-Type: application/json`, header/body caps and read deadline as the other
controls. Its body must be an empty JSON object `{}` (surrounding whitespace is
allowed). Empty bytes, malformed JSON, non-object values or nonempty objects
return `422`. Success is `200` with raw `BatchProgress`, not an envelope or
`batchId`.

- **Stop is a one-way latch on the installed controller.** It sets
  `stopRequested: true`, clears pause intent and wakes the condition variable.
  Repeated stop while that controller remains active returns `200`. Pause and
  in-memory resume after stop return `409` and cannot clear the latch. Idle,
  finished/aborted batches, form-options, admission before installation, and
  worker teardown return `409` without spawning a worker. A stale captured
  controller cannot affect a subsequent batch.
- **An admitted attempt settles; no new attempt is admitted after stop.** Stop
  does not cancel the blocking worker round trip. Submit-attempt admission is
  serialized with the latch; an attempt admitted before stop is in flight even
  if its transport call has not yet returned. Safe retries are new attempts and
  are prohibited after stop. Retry backoff waits on the same condition variable,
  so stop interrupts it immediately rather than waiting for the backoff deadline.
  Pause alone still permits the existing safe retry policy. An indeterminate or
  ambiguous transport result still aborts exactly once, without retry/requeue.
- **Results remain truthful.** A completed request retains its actual outcome and
  attempt count. A provably-unsent transient error stopped before retry is
  `failed` with its actual attempts and original error code/message context, not
  `skipped`, success, or indeterminate. Never-attempted trainees are drained in
  queue order as `skipped`, with zero attempts and message
  `not submitted: operator stop`. Report counts include all rows and
  `report.aborted` is true for an observed stop.
- **Stopping has a durable boundary.** Before dequeuing another trainee (also
  when waking from pause), Rust must persist `status: aborted`, settled results,
  ordered pending IDs, and `in_flight: null` before publishing `state: stopped`.
  The final aborted checkpoint includes the drained skips. Both stop-boundary
  and final stop-checkpoint failures are fatal: no later submission or fallback
  terminal write, and no fabricated successful report. Error progress is
  `stopped`, but may retain the last durable counts; inspect the checkpoint.
  Atomic storage leaves the prior checkpoint intact on a failed replacement.
- **Completion has an ordering point.** Stop and terminal completion are
  serialized. If stop is observed before completion closes the controller, the
  batch is aborted even if the last row succeeded; that row remains a success
  and no nonexistent skipped row is invented. If completion closes first,
  late stop is `409` and cannot relabel a fully processed successful batch.
  The active state (or already durable `paused` state) remains visible while
  stop is pending; requesting stop alone never publishes `stopped`.
- **Cleanup and recovery are distinct.** Controls close on completion, error or
  panic before worker teardown; admission stays busy until worker shutdown ends.
  `stopRequested` remains true in the stopped batch's progress, including an
  error after stop, and resets to false on a newly admitted batch or daemon
  restart. Restart creates no controller. Stop/pause/resume cannot acknowledge
  recovery or replay trainees. The existing guard still refuses a fresh start
  over landed/uncertain work. Only explicit reviewed `POST /batch` with
  `resume: true` acknowledges recovery and creates a new worker when there are
  never-attempted rows to run; settled/uncertain rows are carried, never rerun.

The CLI still has no controller and its behavior is unchanged. This stage uses
only synthetic offline tests, not live portal acceptance. The thin extension
sends Stop once and polls Rust; it never cancels a worker, requeues trainees or
acknowledges recovery. While stop is latched, new Start, session edits and
Pause/Resume remain locked. Stopped progress with no report requires human review.

### Offline verification

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```

The socket tests inject fixture transports. Binary/CLI integration tests always
set `DSSP_WORKER_PY` to `tests/fixtures/worker.py`, which reads only synthetic JSON
and never imports `app.worker`, launches a browser, or opens a network. All
mutable fixture input, tokens, checkpoints and protocol traces live in temporary
scratch directories, not operator job/evidence/marker locations. Stage 30 tests
cover a blocked submission with pending pause, durable safe-boundary publication,
responsive polling, same-worker resume without duplicates, idempotent/cancelled
intent, controller cleanup, shared request security, paused recovery refusal with
zero worker starts, and fatal pause/resume checkpoint failures. Stage 31 adds
blocked-submit stop with exactly-once settlement, paused wake without another
POST, interruptible long backoff with no retry, immutable stop intent,
idempotence, stale-controller refusal/fresh-controller reset, idle/admission/
form-options refusal, preserved checkpoint information on stop write failure,
ambiguous/indeterminate abort, shared control security/caps/deadlines, and
explicit-only recovery without replay. Engine tests cover both boundary and
terminal stop write failures and stop observed before last-row completion.

Stage 31 offline validation: **215 tests passed** (211 unit/loopback tests and
4 fixture binary/CLI tests); Rustfmt and Clippy `--all-targets -- -D warnings`
passed. The full unfiltered Rust suite subsequently completed successfully in
one run. Earlier validation used complementary runs after a 120-second limit:

```sh
cargo test --offline -- --skip engine::tests::probe_crash_matrix
cargo test --offline engine::tests::probe_crash_matrix
```

## Safety

- An **indeterminate** submission is never retried automatically.
- An **ambiguous** trainee name never resolves to a guess.
- A batch **never starts over** an unfinished one without you saying so: the
  record of a submission that may have landed is refused over, not overwritten.
- No credentials are stored — auth lives only in the browser profile; nothing
  sensitive is logged.
