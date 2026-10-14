# Stage 29 — Extension → Rust daemon API

Status: **implemented; Stages 29–31 Passed on offline fixture acceptance.** The Rust
daemon and thin extension client follow this contract. See `rust/README.md` for
configuration, HTTP wire shapes, and platform limitations.

Stages 15/24 established one approved ordinary session through the single Rust
chain. That run did **not** verify live batch execution, checkpoint recovery, or
final assessments. Stage 29 uses synthetic workers and jobs only; the former
"first honest run" wording must not be read as a requirement to submit a live
batch or as a claim that such a batch was verified.

## Why a daemon

The extension is the FACE (UI only); Rust is the BRAIN (jobs, queue, state,
retry, dedupe, checkpointing, recovery, every decision). Stage 29 is the wire
between them, and three transports were considered:

- **Native messaging** — rejected. The browser owns the native host's lifetime,
  so an MV3 service-worker eviction kills an in-flight batch. That inverts the
  whole architecture: Rust is supposed to own job state, not depend on a browser
  tab staying interested.
- **WebSocket** — rejected. Live push is the only thing it buys, at the cost of
  framing code or a dependency. `rust/Cargo.toml` currently carries only
  `serde`, `serde_json` and `uuid`; the spec says not to introduce unnecessary
  frameworks.
- **A localhost HTTP daemon with a bearer token** — chosen. The process lifetime
  is the operator's, not the browser's; the popup is a thin client; and the
  dependency cost is zero, because HTTP/1.1 on `std::net` is a few hundred lines
  for this surface. It is also the only one of the three that is a *real attack
  surface*, which is why the security section below is not optional.

## API

One process, bound to `127.0.0.1` only. Every endpoint takes
`Authorization: Bearer <token>` and rejects anything without it.

The surface is not invented: it is `src/core/infrastructure/messaging/Messages.ts`'s
`Message` union, one endpoint per member, so the popup's existing call sites keep
their meaning.

| method | path | maps to | returns |
|---|---|---|---|
| `GET` | `/status` | `GET_STATUS` | `BatchProgress` |
| `GET` | `/report` | `GET_REPORT` | `BatchReport` or `null` |
| `GET` | `/checkpoint` | `GET_CHECKPOINT` | `BatchCheckpoint` or `null` |
| `GET` | `/form-options` | `GET_FORM_OPTIONS` | `TrainingFormOptions` |
| `POST` | `/batch` | `START_AUTOMATION` | `202` + batch id, or `409` refusal |
| `POST` | `/batch/pause` | `PAUSE_AUTOMATION` | `200` + raw progress with pause intent; `409` if no controllable batch |
| `POST` | `/batch/resume` | `RESUME_AUTOMATION` | `200` + raw progress; same in-memory batch only |
| `POST` | `/batch/stop` | `STOP_AUTOMATION` | `200` + raw progress with latched stop intent; `409` if no controllable batch |

`POST /batch` body:

```json
{
  "trainees": [{ "id": "123" }, { "name": "Jane Roe" }],
  "session": {
    "training_date": "2026-09-14",
    "instructor": "Jane Smith",
    "training_type": "Practical"
  },
  "resume": false
}
```

Request fields map to existing `TraineeRef`, `SessionInput`, and checkpoint
machinery. Responses are direct JSON, not extension message envelopes.
`BatchProgress` preserves the TypeScript field names, including optional
`currentTraineeName`. Report/checkpoint HTTP keys are camelCase; timestamps are
epoch-millisecond strings. Checkpoints expose `inFlight` and may be `aborted`.
The popup preserves these fields and formats timestamps without maintaining its
own recovery or execution state.

The status codes carry the same meanings as the CLI's exit codes, because they
answer the same questions:

| code | meaning | CLI equivalent |
|---|---|---|
| `202` | started; nothing about it is known yet | — |
| `409` | the start was refused — an unfinished predecessor (`recovery::guard`). Nothing was spawned. | exit 3 |
| `422` | the batch could not be resolved. Nothing was submitted. | exit 2 |
| `503` | a batch is already running | — |

## The six obligations

**1. `recovery::guard` on the start path, before anything is spawned.**
`POST /batch` reads the checkpoint slot first, exactly as `run_batch` does:
`recovery::guard(&store, req.resume)` is called *before* the worker is spawned
and before the portal is contacted. A refusal becomes `409` with the same text
and counts the CLI prints, and no process is started. This is the daemon's whole
obligation, and it is the one thing that must be tested by driving the daemon
into the same refusal the CLI produces.

**2. The "one way in" claim is settled in the runbook, not in code.** A4 chose
the documented trade: `run_single` stays stateless and warns before it submits.
That is not re-opened here. The daemon guards its own start path (obligation 1);
it is the second start path, not the only ungated one.

**3. `DSSP_RESUME` becomes a per-request field.** The CLI reads it from the
environment (`recovery::resume_requested()`), which is fine for a CLI and wrong
for a daemon: the environment is process-wide, so a browser button press would
silently inherit whatever the operator last exported. `guard` already takes the
acknowledgement as a `bool` — the daemon passes `req.resume` and **never calls
`resume_requested()`**. At startup it logs one line if `DSSP_RESUME` is set in
its own environment, saying that it is ignored for HTTP requests, so the
behaviour is visible rather than surprising.

**4. The extension holds no batch state.** `BatchProgress` is rendered *from*
`GET /status`; `GET /checkpoint` proxies the daemon's own read of the file. Any
copy of progress, checkpoint, or queue state in `service-worker.ts` is a
competing source of truth and must be deleted, not synchronised. The extension's
job is to display and to send intent.

**5. Localhost is a real attack surface.** Any page in any browser on the
machine can reach `127.0.0.1`. The measures:

- **Token**: 32 bytes / 256 bits of CSPRNG output from `/dev/urandom`,
  hex-encoded on first run and written to an owned `0600` file beside the profile.
  Symlinks, hardlinks, unsafe permissions and malformed files are refused.
  `dssp.daemon.token` is gitignored; the token value is never logged or returned. The
  operator pastes it into the extension once; the extension stores it in its own
  storage. There is deliberately **no auto-discovery** — a discovery mechanism
  would hand the token to anything that could read it.
- **Origin**: reject any request carrying an `Origin` that is not the
  extension's own (`chrome-extension://<id>`, `moz-extension://<uuid>`). An
  absent `Origin` is allowed only alongside a valid token, for the CLI and
  `curl`.
- **CORS**: implement none. No `Access-Control-Allow-Origin` header, ever, and
  no wildcard. Without it a page cannot read a response, which is the default
  posture we want rather than one we have to enforce.
- **The forged no-cors POST**: the dangerous case is a page causing an *effect*
  it cannot read. Requiring `Authorization` **and** `Content-Type:
  application/json` makes the request non-simple, so the browser must preflight
  it first — and we never answer a preflight with an allow. The effect is
  therefore blocked by the browser before the token is even considered. The
  token is the second line, not the first.
- **Binding and DoS**: `127.0.0.1` only, never `0.0.0.0`. Cap request bodies
  (64 KB is far above anything this API needs), set read timeouts, and answer
  `405` to anything not listed above. Port: a fixed default with an env
  override, because the extension has to know where to look.
- **Threading**: the batch runs on its own thread, so `GET /status` keeps
  answering while a batch is running. A single-threaded accept loop that ran the
  batch inline would make the popup hang for the length of the run — which is
  also the run it most needs to watch. `std::thread` is enough; no runtime.

**6. MV3 and Firefox lifecycle.** The daemon owns the batch, so a service-worker
eviction is harmless: the popup re-opens, polls `/status`, and finds the batch
where it left it. That is the point of choosing this transport, and it is why
the extension needs no keepalive. What differs between browsers is how the popup
talks to storage and runtime (`browser.*` promise-style versus `chrome.*`
callback-style) — ADR 0001 decision 3's `FirefoxBrowserAdapter` seam, which
Stage 37 requires. When the daemon is not running, the popup shows that plainly
and prints the command to start it. **The extension never spawns the daemon**:
owning the process lifetime from the browser is exactly the property that got
native messaging rejected.

## Acceptance for the build

- The daemon refuses a start over a live checkpoint exactly as the CLI does —
  a test that drives it into the same refusal, not a reading of the code.
- `cargo run -- job.json` still works unchanged, because the live gates depend
  on it and the daemon must not become the only way to run a batch.
- No TypeScript module gains a second copy of batch state; `pnpm typecheck &&
  pnpm test` stay green.
- The security measures above are tested, not asserted: a request with no token,
  a request with a foreign `Origin`, and a request with a simple content type
  are each rejected.

## Offline acceptance results

- `cargo test --offline`: **193 passed** (189 coordinator/loopback tests,
  including 27 daemon tests; 4 fixture-backed binary/CLI integration tests).
- `cargo clippy --offline --all-targets -- -D warnings`: passed.
- `pnpm typecheck`: passed; `pnpm test`: **223 passed**.
- Recovery refusal is driven over HTTP and produces `409` with zero worker
  starts. Explicit resume, ignored `DSSP_RESUME`, concurrent admission, responsive
  polling, request caps/timeouts, malformed framing, authentication and Origin
  rejection are exercised. Worker teardown retains admission until shutdown;
  panic/error paths release connection slots and preserve uncertain checkpoints.
- The active extension no longer imports the TypeScript engine, stores execution
  state, or injects portal content scripts. It stores only manually supplied
  connection settings; it never starts the daemon or retries batch admission.
- An existing legacy forwarding test was corrected to call each adapter with its
  actual typed arguments; no legacy production portal behavior was changed.

## Stage 30 — cooperative pause and in-memory resume

Both control endpoints require the existing bearer token, allowed Origin, JSON
content type, and an empty JSON object `{}`. They share request caps/read
limits. A valid control returns `200` with raw `BatchProgress`, including
`pauseRequested`; it never returns a new batch ID or admits another worker.

- Pause is a request, not cancellation. During a trainee's submission, retries
  allowed by the existing safe policy, and settlement, the active state remains
  visible with `pauseRequested: true`. Never cancel/requeue an in-flight POST.
- With work still pending, the engine persists a paused checkpoint **before
  dequeuing** the next trainee and before publishing `state: paused`. Settled
  results and queue order are preserved; `in_flight` is null. Rust waits on a
  condition variable; status polling stays responsive and new starts return `503`.
- Resume clears pause intent and wakes the same engine/worker. The response can
  remain `paused` until Rust durably writes `running`. A failed pause or resume
  checkpoint halts before any later submission. Unknown outcomes still abort;
  exhausted-queue completion wins over pause.
- Repeated pause/resume is idempotent within the same controllable batch. Resume
  can cancel a pending pause. Admission before controller installation, idle,
  finished/aborted, teardown, and form-options work return `409` without changing
  execution. Stale controllers cannot alter another batch.
- These controls do **not** acknowledge restart recovery. After restart, a paused
  checkpoint has no live controller: `/batch/resume` returns `409`, and ordinary
  batch admission is still guarded. Only reviewed `POST /batch` with explicit
  `resume: true` and the original session template acknowledges recovery.
- The popup sends intent once, then polls Rust. Session edits and new admission
  stay locked while pause is pending/active. It distinguishes pending pause,
  durable pause, and wake-up; it stores no queue/recovery state. Lost/refused
  control requests are shown, not automatically retried. Stage 31's Stop semantics
  are documented below; a latched stop prevents further Pause/Resume controls.

### Stage 30 acceptance results

- `cargo test --offline`: **205 passed** (201 coordinator/loopback tests plus
  4 fixture binary/CLI tests); Clippy `--all-targets -- -D warnings` passed.
- `pnpm typecheck`: passed; `pnpm test`: **256 passed** across 20 files.
- Deterministic fixtures cover pending pause during a blocked submission,
  exactly-once settlement, unchanged pending queue, responsive polling, same-worker
  resume, cancellation/idempotence, indeterminate abort, fatal checkpoint failures,
  restart refusal with zero worker starts, and controller cleanup.
- Mocked UI/client tests cover authenticated JSON control bodies, `200` versus
  admission `202`, stale polling responses, disconnect/reconnect, disabled controls,
  no automatic retries/fallback, and the distinction from checkpoint recovery.

All checks are offline; no real training records were submitted for this stage.

## Stage 31 — cooperative, latched stop

`POST /batch/stop` requires authenticated, Origin-validated JSON `{}` and shares
all request caps/read deadlines with Pause/Resume. It returns `200` raw progress
with `stopRequested: true`, not a new batch ID or proof that execution has ended.
Idle, finished/aborted, form-options work, teardown and admission before controller
installation return `409`; malformed/nonempty control bodies return `422`.

- Stop is latched for that controller. It clears pause intent, wakes a paused
  engine, and interrupts retry backoff. Repeated pending Stop is idempotent;
  Pause/Resume after Stop return `409` and cannot undo it. Stale controllers
  cannot control a newly admitted batch.
- Already admitted worker requests settle without cancellation. Submit-attempt
  admission is serialized with Stop: no new attempts, including safe retries,
  are admitted afterward. This does not claim that an already admitted request
  can be cancelled before its transport call. Unknown responses still abort
  without retry or requeue.
- Actual outcomes and attempts remain intact. A provably-unsent transient error
  stopped before retry remains failed with its original error context and actual
  attempts. Never-attempted trainees become ordered zero-attempt skips with
  `not submitted: operator stop`; counts account for the whole roster.
- Rust persists an aborted stop boundary with settled results, ordered pending
  IDs and null `in_flight`, then an aborted terminal checkpoint with drained skips.
  Both writes are fatal on failure: no later submission or fabricated successful
  report. Error progress can be stopped with a null report and only last-durable
  counts, so human review remains necessary.
- Stop and completion are serialized. If Stop wins, the report is aborted even
  if the last row succeeded, preserving that success without inventing skips.
  If completion closes first, late Stop is `409`. Requesting Stop alone never
  publishes a terminal stopped state; status remains responsive while settling.
- Stop intent remains visible after terminal/error progress and resets for a new
  batch or daemon restart. Controls do not acknowledge recovery. Ordinary starts
  over landed/uncertain work are still refused; only separately reviewed batch
  admission with explicit `resume: true` acknowledges checkpoint recovery.
- The thin popup sends Stop once, polls Rust, and distinguishes pending Stop from
  terminal stopped/aborted. While Stop is latched, Pause/Resume, new Start and
  session edits remain disabled, including terminal stopped. The UI does not
  provide recovery admission; external reviewed admission is still guarded by
  Rust. Missing reports and stale/disconnected views never cause auto-retry.

### Stage 31 acceptance results

- `cargo fmt --check`: passed; full `cargo test --offline`: **215 passed**
  (211 coordinator/loopback plus 4 fixture binary/CLI tests).
- `cargo clippy --offline --all-targets -- -D warnings`: passed.
- `pnpm typecheck`: passed; `pnpm test`: **313 passed** across 20 files.
- Deterministic fixture coverage includes blocked-request settlement exactly once,
  no later trainee/retry, paused wake-up, interrupted backoff, irrevocable intent,
  completion races, checkpoint-write failure, stale-controller refusal, security
  checks and explicit-only recovery. Mocked client/UI tests cover authenticated
  encoding, exact `200` acceptance, no automatic retry/fallback, pending/terminal
  presentation, latched controls, stale responses, reconnect and missing reports.

No live portal/browser requests or training submissions were used. Normal CLI
execution still has no controller and retains its existing behavior.

## Remaining operational limits

- Token-file hardening is Linux-specific. Installed-browser Origin/permission
  behavior and Firefox remain later integration/browser checks, not claims made
  by mocked-fetch tests.
- Do not run another CLI/daemon against the same profile/checkpoint concurrently:
  no cross-process ownership lock exists. Only the checkpoint survives restart;
  in-memory progress/report reset and the start guard reads the durable file.
- Worker shutdown has no deadline; a stuck worker retains admission (fails
  closed). The 16-handler cap limits resource use but does not promise polling
  availability during saturation.
- Stage 29 does not establish live batch safety or final-assessment support.

## Not in this design

- **Forced cancellation/termination.** Stop is cooperative: it does not interrupt
  a worker request already admitted or undo a portal record. Hung requests and
  worker shutdown still need manual investigation, never blind restart/replay.
- **Push instead of poll.** The popup polls `/status` while it is open. A push
  channel is a WebSocket by another name and was rejected with it; if polling
  proves too coarse, that decision can be revisited on its own merits.
- **Automatic daemon start** (launchd/systemd/`chrome.runtime.connectNative`).
  Every version of it hands the browser control of the process lifetime.
