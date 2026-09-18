# DSSP-Bot Worker Protocol (v1)

Wire protocol between the **Rust coordinator** and the **Python worker**.

## Transport

- Rust spawns the worker as a subprocess:
  `python/.venv/bin/python -m app.worker` (working directory `python/`).
- Communication is **newline-delimited JSON** (one object per line) over the
  child's **stdin/stdout** pipes.
- **stdout carries protocol lines only.** All worker logging goes to **stderr**.
  Rust must treat any non-JSON stdout line as a protocol violation.
- On startup the worker emits the readiness handshake before reading anything:

  ```json
  {"v":1,"status":"ready"}
  ```

- Rust closes stdin to shut the worker down; the worker exits 0 after closing
  its browser context.

## Envelope

Every request:

```json
{"v":1,"job_id":"<uuid4>","op":"<operation>", ...op fields}
```

Every response:

```json
{"v":1,"job_id":"<same id>","op":"<operation>","status":"ok", ...op fields}
```

or

```json
{"v":1,"job_id":"<same id>","op":"<operation>","status":"error",
 "error_code":"<CODE>","message":"<human readable>",
 "proves_nothing_submitted":true|false}
```

**`job_id` is echoed on every response** and must stay consistent end to end.

## Division of responsibility

- **Python reports what happened; Rust decides what to do about it.** The worker
  never retries, never resubmits, never decides retryability.
- `proves_nothing_submitted` is a hint computed from the error code (see table).
  When `true`, the failure occurred before anything could reach the portal, so
  re-invoking the op is safe. When `false` (e.g. `NETWORK`, `TIMEOUT`), the
  request may have landed — Rust must treat it like an indeterminate outcome,
  not a clean failure.
- `submit_training` commits **at most once** per invocation. It performs
  prepare (GET + parse) → commit (single POST) → classify, and reports the
  classified outcome. It is never re-invoked for the same trainee by Rust
  unless the failure proves nothing was submitted.

## Operations

### `ping`

Request: `{}` — Response: `{"pong":true}`

Liveness check. Does not touch the browser or the portal.

### `ensure_session`

Request: `{}` — Response: `{"authenticated":true}`

Opens the portal's `/Trainee` page in the persistent Chromium context. If the
login page is shown, the worker **waits** (polling up to 5 minutes by default)
for the operator to sign in manually in the opened window. The session is
persisted in the browser profile (`python/.pw-profile/`, gitignored), so login
is normally needed once per profile, not per run.

Errors: `SESSION_EXPIRED` (login timeout), `NETWORK` (portal unreachable).

### `list_trainees`

Request: `{}` — Response: `{"count":N,"trainees":[{"id","name","sn","course","training_sessions"}]}`

Fetches the full trainee list via the authenticated context.

### `get_form_options`

Request: `{"trainee_id":"123"?}` — Response: `{"trainee_id":"123","instructors":[{"value","label"}],"training_types":[{"value","label"}]}`

Loads the Log New Training form and returns its instructor/training-type
options. Without `trainee_id`, uses the first trainee (the options are the same
for every trainee).

### `submit_training`

Request:

```json
{"trainee":{"id":"123"} or {"name":"John Doe"},
 "session":{"training_date":"YYYY-MM-DD (or DD/MM/YYYY)",
            "instructor":"<value or label>",
            "training_type":"<value or label>"}}
```

- Trainee resolution: by `id` when given; otherwise by **normalized name**
  (whitespace-collapsed, case-insensitive). Two trainees with the same
  normalized name → `TRAINEE_NOT_FOUND` with an "Ambiguous" message — the
  worker never guesses.
- `instructor`/`training_type` match by value **or** label (mirrors the old
  `selectFormOption`).

Response (status `ok` — the op ran; the classification is in `outcome`):

```json
{"trainee":{"id":"123","name":"..."},"attempts":1,
 "outcome":"confirmed|duplicate|rejected|indeterminate",
 "reference":"<url>"?, "message":"..."?}
```

| outcome | meaning | Rust action |
|---|---|---|
| `confirmed` | portal accepted the record | done |
| `duplicate` | portal says already logged | done (report) |
| `rejected` | portal refused (validation/HTTP) | done (report reason) |
| `indeterminate` | submitted but result unreadable | **STOP — never resubmit**; require investigation |

## Error codes

Mirrors the legacy extension's `ErrorCode` taxonomy, plus protocol-level codes.

| code | meaning | `proves_nothing_submitted` |
|---|---|---|
| `ELEMENT_NOT_FOUND` | portal selector missing | true |
| `TIMEOUT` | operation timed out (may have landed) | false |
| `NETWORK` | transport failure (may have landed) | false |
| `PORTAL_UNAVAILABLE` | portal unreachable | true |
| `SESSION_EXPIRED` | login page / login timeout | true |
| `TRAINEE_NOT_FOUND` | no match, or **ambiguous name** | true |
| `MISSING_DATA` | required data absent | true |
| `VALIDATION_FAILED` | bad input (date format, option not in list) | true |
| `DUPLICATE_RECORD` | (reserved; duplicates surface as `outcome`) | — |
| `PORTAL_STRUCTURE_CHANGED` | form shape unexpected (e.g. not POST) | true |
| `PORTAL_NOT_MAPPED` | (reserved) | true |
| `CONFIRMATION_UNKNOWN` | (reserved; surfaces as `outcome:"indeterminate"`) | false |
| `SUBMISSION_FAILED` | unexpected worker failure | false |
| `BAD_REQUEST` | malformed protocol request | true |
| `UNKNOWN_OP` | unrecognized `op` | true |

## Safety rules enforced by this protocol

1. **Never retry an indeterminate submission** — `outcome:"indeterminate"`
   means the record may exist; only a human may resolve it.
2. **Never automatically resolve an ambiguous trainee** — the worker stops and
   asks for the id.
3. **Never store credentials** — auth lives only in the gitignored browser
   profile; no passwords/cookies/tokens in code or logs.
4. **`job_id` consistency** — echoed on every response line.
5. **Golden rule** — Rust never fixes an automation error by adding retries;
   retryability comes from `proves_nothing_submitted` / the outcome table.

## Configuration (env, read by the worker)

| var | default | purpose |
|---|---|---|
| `DSSP_ORIGIN` | `https://dssp.frsc.gov.ng` | portal origin override (tests use this) |
| `DSSP_PROFILE_DIR` | `python/.pw-profile` | persistent Chromium profile |
| `DSSP_HEADLESS` | `0` | `1` runs Chromium headless |
| `DSSP_LOGIN_TIMEOUT_MS` | `300000` | how long `ensure_session` waits for manual login |
| `DSSP_NAV_TIMEOUT_MS` | `30000` | per-request navigation timeout |
