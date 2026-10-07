# Stage 29 — Extension → Rust daemon API

Status: **designed, not implemented.** Stage 29 is `⬜ Not Started` in the
runbook's status table. This document exists so the API is settled before code
is written, following the same discipline as `docs/protocol.md`: the contract is
recorded, then implemented, not the other way round.

The daemon is **not** built until the live gates have run. Building it first
would put a UI on top of batch machinery that has never once been exercised
against the real portal — the batch path's first honest run should not also be
the first proof of two stages at once.

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
| `POST` | `/batch/pause` | `PAUSE_AUTOMATION` | reserved — Stage 30 |
| `POST` | `/batch/resume` | `RESUME_AUTOMATION` | reserved — Stage 30 |
| `POST` | `/batch/stop` | `STOP_AUTOMATION` | reserved — Stage 31 |

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

Every field maps onto a type that already exists — `TraineeRef`,
`SessionInput`, and the checkpoint store. `BatchProgress` is emitted exactly as
`Messages.ts:7` declares it, including `currentTraineeName`, so the popup's
`renderProgress` (`src/popup/main.ts:94`) needs no reworking.

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

- **Token**: 32+ bytes of CSPRNG output (`uuid::Uuid::new_v4` twice is 244 bits),
  generated on first run and written to a `0600` file beside the profile. The
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

## Not in this design

- **Pause / resume / stop** (Stages 30, 31). The endpoints are reserved so the
  `Message` union maps cleanly, but the engine has no API for them yet and the
  spec forbids building ahead of the stage.
- **Push instead of poll.** The popup polls `/status` while it is open. A push
  channel is a WebSocket by another name and was rejected with it; if polling
  proves too coarse, that decision can be revisited on its own merits.
- **Automatic daemon start** (launchd/systemd/`chrome.runtime.connectNative`).
  Every version of it hands the browser control of the process lifetime.
