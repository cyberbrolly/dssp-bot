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
  interrupted so the next attempt is ordinary.
- **`DSSP_RESUME=1`** — continue it instead. The never-attempted trainees run;
  everything the predecessor recorded is carried forward into the new file so it
  stays whole. Nothing whose submission may already be on the portal is ever
  submitted again — those are reported and left for you. If there is nothing left
  to attempt, the run exits 0 when every loose end is settled and 3 when a human
  is still owed.
  A file from a build **older than this one** — written before the checkpoint
  named a submission in flight — runs none of its queue: it cannot say how far
  through it got, so every trainee still waiting in it is carried forward as
  unconfirmed and the run exits 3 with the names. Check the portal for them.
- **Unreadable** — a corrupt file is not a file that says nothing ran. Refused,
  never treated as an empty slot.
- **Killed, but owing nothing** — a run that died between its last result and
  its final write. Nothing is in doubt, so it starts clean and reports what it
  recovered. Being killed is not by itself a reason to refuse.

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

## Safety

- An **indeterminate** submission is never retried automatically.
- An **ambiguous** trainee name never resolves to a guess.
- A batch **never starts over** an unfinished one without you saying so: the
  record of a submission that may have landed is refused over, not overwritten.
- No credentials are stored — auth lives only in the browser profile; nothing
  sensitive is logged.
