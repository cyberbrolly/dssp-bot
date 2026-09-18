# DSSP-Bot Core (Rust coordinator)

First-milestone coordinator: spawns the Python Playwright worker, drives one
trainee end to end, and owns every retry/stop decision.

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

## Configuration (environment)

| var | default | purpose |
|---|---|---|
| `DSSP_ORIGIN` | `https://dssp.frsc.gov.ng` | portal origin |
| `DSSP_HEADLESS` | `0` | `1` = run Chromium headless |
| `DSSP_PROFILE_DIR` | `../python/.pw-profile` | persistent login profile |
| `DSSP_LOGIN_TIMEOUT_MS` | `300000` | how long to wait for manual login |
| `DSSP_NAV_TIMEOUT_MS` | `30000` | per-request timeout |
| `DSSP_WORKER_PY` | `../python/.venv/bin/python` | worker interpreter |

## Result & exit codes

The coordinator prints one line to stdout (progress/logs go to stderr):

| line | exit | meaning |
|---|---|---|
| `RESULT: confirmed …` / `RESULT: duplicate …` | 0 | recorded (or already present) |
| `HALT: indeterminate …` | 3 | **submitted but unconfirmed — verify on the portal, do not re-run blindly** |
| `HALT: SESSION_EXPIRED …` | 3 | session lapsed / portal changed — stop |
| `FAILED: rejected …` / `FAILED: TRAINEE_NOT_FOUND …` | 4 | definitive failure, no retry |

## Safety

- An **indeterminate** submission is never retried automatically.
- An **ambiguous** trainee name never resolves to a guess.
- No credentials are stored — auth lives only in the browser profile; nothing
  sensitive is logged.
