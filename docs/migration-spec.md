# DSSP-Bot — Python + Rust Migration Implementation Specification

**Project:** DSSP-Bot
**Migration:** TypeScript → Python + Rust
**Status:** In Progress
**Primary Goal:** Replace the existing TypeScript automation architecture with a reliable Python + Rust system while preserving the useful behavior and reliability principles of the existing implementation.

---

> **Filed 2026-09-19.** This document is the statement of *intent* — what the
> migration is for, and what "done" means. It is **not** the operational
> tracker. Its stage numbers (1–23) are a different decomposition from
> [`migration-runbook.md`](migration-runbook.md) (1–40, with six hard gates),
> and the two do not correspond — do not map one number onto the other.
> Use the runbook's status table to see where the work stands; use this
> document to check whether a stage was done *as intended*. Everything below
> this note is the specification as written.

---

# 1. Mission

Migrate DSSP-Bot from its existing TypeScript browser-extension automation architecture to:

```text
Browser Extension
       │
       ▼
   Rust Core
       │
       ▼
 Python Worker
       │
       ▼
   Playwright
       │
       ▼
 DSSP Portal
```

The migration must be incremental.

**Do not delete the existing TypeScript implementation until the replacement has been tested and proven to work.**

The existing TypeScript implementation should be treated as a reference implementation.

---

# 2. Architecture Responsibilities

## Browser Extension — UI

The extension is responsible for:

* Trainee input
* Date selection
* Instructor selection
* Training type selection
* Start
* Pause
* Resume
* Stop
* Progress display
* Results display
* Error display

The extension must NOT contain DSSP-specific automation logic.

---

## Rust — Core / Brain

Rust owns:

* Job creation
* Job IDs
* Queue
* Batch management
* State machine
* Retry policy
* Deduplication
* Checkpointing
* Recovery
* Error classification
* Results
* Rust ↔ Python communication

Rust decides **what should happen next**.

---

## Python — Browser / Portal Worker

Python owns:

* Playwright
* Browser lifecycle
* Browser context
* DSSP session
* Portal navigation
* Trainee discovery
* Trainee matching
* Form interaction
* Form validation
* Submission
* Submission confirmation
* Portal-specific errors

Python reports **what happened**.

Python must not own the global job queue or retry policy.

---

# 3. Core Reliability Rule

Every training submission must conceptually follow:

```text
PREPARE
   ↓
COMMIT
   ↓
CONFIRM
```

### PREPARE

Safe operations:

* Find trainee
* Open trainee
* Open form
* Fill form
* Validate form

These may generally be retried.

### COMMIT

The actual submission.

Once the submission may have reached DSSP, treat it as committed unless proven otherwise.

### CONFIRM

Verify that the submission actually succeeded.

If confirmation fails after a possible successful submission:

```text
indeterminate
```

Do NOT blindly submit again.

---

# 4. Migration Principles

The coding agent must follow these rules:

1. Work incrementally.
2. Keep the existing TypeScript implementation intact during migration.
3. Do not rewrite everything at once.
4. Do not introduce unnecessary frameworks.
5. Do not duplicate retry/state logic across layers.
6. Rust owns job state.
7. Python owns browser/portal state.
8. Extension owns UI state.
9. Use structured errors.
10. Test every major stage before continuing.
11. Prefer simple implementations over premature abstractions.
12. Never solve an automation problem by blindly adding retries or delays.
13. Never automatically replay an indeterminate submission.
14. Do not add features that are not required for the current migration stage.

---

# 5. Current Repository

The repository contains the existing TypeScript implementation.

Important existing concepts to preserve:

### `PortalAdapter`

The existing implementation already separates portal operations behind an abstraction similar to:

```text
isPortalPage
getTrainees
getFormOptions
openTrainee
openTrainingForm
fillTrainingForm
validateTrainingForm
submitTrainingForm
waitForSubmissionResult
```

Use this as the conceptual model for the Python portal layer.

### Existing Automation Flow

The existing implementation already uses:

```text
prepare
commit
confirm
```

Preserve this reliability model.

### Existing Checkpointing

The existing implementation checkpoints after settled trainee operations.

Move this responsibility to Rust.

---

# 6. Migration Stages

The coding agent MUST work through these stages in order.

---

# Stage 1 — Verify Repository

Before modifying architecture:

```bash
git status
git log --oneline -5
```

Inspect:

```text
python/
rust/
extension/
docs/
```

Also inspect the existing TypeScript implementation.

Do not assume the repository is dirty or clean.

Do not create meaningless commits.

### Acceptance Criteria

* Existing repository state is understood.
* Existing TypeScript implementation is preserved.
* Migration work happens in a dedicated migration branch when appropriate.
* No existing functionality is deleted.

---

# Stage 2 — Create Python + Rust Structure

Create the basic structure:

```text
python/
├── app/
│   ├── main.py
│   ├── browser/
│   ├── models/
│   ├── portal/
│   └── protocol/
└── tests/

rust/
├── Cargo.toml
└── src/
    ├── main.rs
    ├── jobs/
    ├── models/
    ├── state/
    ├── retry/
    └── worker/
```

Keep this structure minimal.

Do not create empty abstractions just for the sake of having many files.

### Acceptance Criteria

* Python project starts.
* Rust project compiles.
* No dependency errors.
* Existing TypeScript implementation still works independently.

---

# Stage 3 — Build Python Worker

Set up Python and Playwright.

The worker must be able to:

1. Start.
2. Launch browser.
3. Create browser context.
4. Create page.
5. Navigate to DSSP.
6. Keep the process alive while performing work.
7. Close browser cleanly.

### Browser Failure Handling

Handle:

* Browser crash
* Page crash
* Navigation failure
* Timeout
* Network failure

Do not automatically retry until the failure has been classified.

### Acceptance Criteria

The following works:

```text
Python
 ↓
Playwright
 ↓
Browser
 ↓
DSSP
```

---

# Stage 4 — Build Python Portal Layer

Create the portal layer based on the old `PortalAdapter` concept.

Suggested responsibilities:

```text
python/app/portal/
├── client.py
├── session.py
├── trainees.py
├── training_form.py
├── submission.py
└── selectors.py
```

Keep selectors separate from business logic where practical.

The portal layer must support:

* Portal detection
* Session detection
* Trainee retrieval
* Trainee matching
* Opening trainee
* Opening training form
* Retrieving form options
* Filling form
* Form validation
* Submission
* Submission confirmation

---

# Stage 5 — Trainee Matching

Implement safe trainee matching.

Matching priority:

```text
1. Trainee ID
2. Normalized name
```

Normalize:

* Capitalization
* Whitespace
* Other safe formatting differences

Example:

```text
Input:
John   Doe

Portal:
JOHN DOE
```

These should match.

However:

```text
JOHN DOE
JOHN DOE
```

must be treated as ambiguous.

Do NOT automatically choose one.

### Possible results

```text
matched
not_found
ambiguous
```

### Acceptance Criteria

* Correct trainee is matched.
* Missing trainee is reported.
* Duplicate names are detected.
* Wrong trainee is never silently selected.

---

# Stage 6 — One-Trainee End-to-End Test

This is the first major milestone.

The system must perform:

```text
Rust
 ↓
Python
 ↓
Playwright
 ↓
DSSP
 ↓
Find one trainee
 ↓
Open form
 ↓
Fill form
 ↓
Validate
 ↓
Submit
 ↓
Confirm
 ↓
Python result
 ↓
Rust result
```

Do not proceed to complex batch functionality until this works.

### Acceptance Criteria

One real trainee can be processed successfully and the result reaches Rust.

---

# Stage 7 — Define Rust ↔ Python Protocol

Create a small, explicit protocol.

Start with one operation:

```text
log_training
```

Example request:

```json
{
  "type": "log_training",
  "job_id": "abc123",
  "trainee_id": "12345",
  "date": "2026-09-18",
  "instructor": "John Doe",
  "training_type": "Practical"
}
```

Example response:

```json
{
  "job_id": "abc123",
  "status": "confirmed",
  "trainee_id": "12345"
}
```

Every request MUST contain a `job_id`.

The response MUST contain the same `job_id`.

---

# Stage 8 — Structured Protocol Errors

Define structured errors rather than relying on arbitrary strings.

Initial error categories:

```text
MATCH_NOT_FOUND
AMBIGUOUS_TRAINEE
SESSION_EXPIRED
PORTAL_UNAVAILABLE
FORM_INVALID
SUBMISSION_REJECTED
SUBMISSION_INDETERMINATE
WORKER_TIMEOUT
WORKER_CRASH
NETWORK_ERROR
```

Each error should communicate enough information for Rust to make a decision.

Example:

```json
{
  "job_id": "abc123",
  "status": "error",
  "error": {
    "code": "SESSION_EXPIRED",
    "message": "DSSP session has expired",
    "retryable": false
  }
}
```

Rust should not need to parse human-readable error messages to determine behavior.

---

# Stage 9 — Rust Job Model

Create the job model.

A job represents one trainee operation.

Example:

```text
Job
├── job_id
├── trainee_id
├── date
├── instructor
├── training_type
├── state
├── attempts
└── result
```

Possible job states:

```text
pending
running
completed
failed
indeterminate
```

Keep the model simple.

---

# Stage 10 — Rust Queue

Rust owns the queue.

Example:

```text
10 trainees
     ↓
Rust Queue
     ├── pending
     ├── running
     ├── completed
     ├── failed
     └── indeterminate
```

Implement:

* Add job
* Get next pending job
* Mark running
* Mark completed
* Mark failed
* Mark indeterminate
* Prevent duplicate jobs

Python must not maintain the global queue.

---

# Stage 11 — Rust Coordinator

Create a coordinator responsible for executing jobs.

Conceptually:

```text
Coordinator
    ↓
get next job
    ↓
send to Python
    ↓
receive result
    ↓
classify result
    ↓
update job
    ↓
checkpoint
    ↓
next job
```

The coordinator should not contain portal-specific logic.

---

# Stage 12 — Retry Policy

Rust owns retry decisions.

Python only reports the result.

Example:

```text
NETWORK_ERROR
      ↓
Rust
      ↓
retry
```

But:

```text
SUBMISSION_REJECTED
      ↓
Rust
      ↓
do not retry
```

And:

```text
SUBMISSION_INDETERMINATE
      ↓
Rust
      ↓
STOP / INVESTIGATE
```

Never automatically retry a potentially committed submission.

Implement:

* Maximum attempts
* Retryable errors
* Non-retryable errors
* Backoff
* Attempt tracking

Do not implement separate retry policies in Python and the extension.

---

# Stage 13 — Checkpointing

Rust owns checkpoints.

Save state after each settled job.

Example:

```json
{
  "batch_id": "batch-123",
  "status": "running",
  "completed": [
    "1",
    "2",
    "3"
  ],
  "pending": [
    "4",
    "5"
  ],
  "indeterminate": []
}
```

Checkpoint information must allow recovery after:

* Rust crash
* Python crash
* Browser crash
* Extension restart

### Recovery

After restart:

```text
Load checkpoint
      ↓
Skip completed jobs
      ↓
Recover pending jobs
      ↓
Investigate indeterminate jobs
```

Never automatically replay an indeterminate job.

---

# Stage 14 — Browser Extension → Rust

Only after Rust + Python work independently.

The extension becomes a UI client.

Architecture:

```text
Extension
    ↓
Rust API
    ↓
Rust Coordinator
    ↓
Python Worker
    ↓
DSSP
```

Implement:

```text
START
GET_STATUS
PAUSE
RESUME
STOP
GET_RESULTS
```

The extension should display the state maintained by Rust.

It should not maintain a competing source of truth.

---

# Stage 15 — Pause / Resume / Stop

Rust owns batch state.

Example:

```text
IDLE
 ↓
RUNNING
 ↓
PAUSED
 ↓
RUNNING
 ↓
COMPLETED
```

Stop:

```text
RUNNING
 ↓
STOPPING
 ↓
STOPPED
```

Prefer pausing between jobs.

Do not terminate a submission halfway through without determining whether the operation was committed.

---

# Stage 16 — Error Handling

The system must classify failures by layer.

```text
Extension
    ↓
UI/API problem

Rust
    ↓
Queue/state/retry problem

Python
    ↓
Worker/protocol problem

Playwright
    ↓
Browser problem

DSSP
    ↓
Portal/session/form problem
```

When troubleshooting:

```text
1. Reproduce
2. Read actual error
3. Identify layer
4. Check logs
5. Check current state
6. Determine whether submission committed
7. Decide retry/stop/recover
8. Fix root cause
9. Re-run smallest test
10. Add regression test
```

---

# Stage 17 — Logging

Logs must make it possible to determine:

```text
What happened?
Which job?
Which trainee?
Which attempt?
Which component?
What was the result?
Can it safely be retried?
```

Example:

```text
INFO job=abc123 trainee=123
     action=submit
     result=confirmed
```

Example failure:

```text
ERROR job=abc124 trainee=456
      action=submit
      error=session_expired
      retryable=false
```

Never log:

* Passwords
* Session cookies
* Authentication tokens
* Other sensitive credentials

---

# Stage 18 — Remove Old TypeScript Automation

Do NOT remove old automation until the replacement has passed:

```text
[ ] One trainee
[ ] Multiple trainees
[ ] Queue
[ ] Retry
[ ] Checkpoint
[ ] Recovery
[ ] Pause
[ ] Resume
[ ] Stop
[ ] Extension integration
[ ] Error handling
```

Then identify which old files can safely be removed.

Potential candidates include the old:

* Automation engine
* Batch runner
* Remote portal adapter
* Service-worker automation
* Main-world interception

Do not delete useful concepts merely because the implementation changed.

---

# Stage 19 — Testing

Test normal operation:

```text
[ ] One trainee
[ ] Multiple trainees
[ ] Multiple batches
```

Test failure scenarios:

```text
[ ] Trainee not found
[ ] Ambiguous trainee
[ ] Session expires
[ ] Network disappears
[ ] Browser crashes
[ ] Python crashes
[ ] Rust crashes
[ ] Portal rejects submission
[ ] Confirmation fails
[ ] Worker timeout
```

Test controls:

```text
[ ] Pause
[ ] Resume
[ ] Stop
```

Test recovery:

```text
[ ] Restart after checkpoint
[ ] Completed jobs are skipped
[ ] Pending jobs continue
[ ] Indeterminate jobs aren't replayed
```

---

# Stage 20 — Critical Reliability Test

The most important failure scenario is:

```text
DSSP accepts submission
        ↓
Browser/Python crashes
        ↓
Rust never receives confirmation
```

The system MUST NOT assume:

```text
"submission failed"
```

and submit again automatically.

Instead:

```text
unknown result
      ↓
indeterminate
      ↓
safe investigation
```

This is necessary to prevent duplicate records.

---

# Stage 21 — Firefox

Only after the Chromium flow is stable.

Test:

```text
[ ] Extension loads
[ ] Extension communicates with Rust
[ ] Python worker works
[ ] Browser session works
[ ] DSSP loads
[ ] Trainee matching works
[ ] Form works
[ ] Submission works
[ ] Confirmation works
```

Firefox support should not destabilize the working Chromium implementation.

---

# Stage 22 — Final Cleanup

After all functionality works:

* Remove temporary debugging
* Remove hardcoded trainee names
* Remove unused TypeScript code
* Remove unused dependencies
* Clean logs
* Clean configuration
* Update README
* Update architecture documentation
* Update protocol documentation
* Update troubleshooting documentation
* Add final tests
* Review security
* Review error handling

---

# 23. Final Architecture

The completed system should look like:

```text
                         USER
                           │
                           ▼
                  ┌─────────────────┐
                  │ Browser         │
                  │ Extension       │
                  │                 │
                  │ UI / Controls   │
                  └────────┬────────┘
                           │
                           │ API / IPC
                           ▼
                  ┌─────────────────┐
                  │ Rust Core       │
                  │                 │
                  │ Jobs            │
                  │ Queue           │
                  │ State           │
                  │ Retry           │
                  │ Checkpoints     │
                  │ Results         │
                  └────────┬────────┘
                           │
                           │ JSON
                           ▼
                  ┌─────────────────┐
                  │ Python Worker   │
                  │                 │
                  │ Playwright      │
                  │ Browser         │
                  │ Session         │
                  │ Portal Client   │
                  │ Form Automation │
                  └────────┬────────┘
                           │
                           ▼
                  ┌─────────────────┐
                  │ DSSP Portal     │
                  └─────────────────┘
```

## Responsibility Rule

```text
Extension = FACE
Rust      = BRAIN
Python    = HANDS
DSSP      = TARGET
```

---

# 24. Definition of Done

The migration is complete when:

```text
[ ] Rust starts successfully
[ ] Python worker starts successfully
[ ] Playwright launches
[ ] DSSP session works
[ ] Trainees can be retrieved
[ ] Trainees are safely matched
[ ] Training forms can be filled
[ ] Submissions work
[ ] Submissions are confirmed
[ ] Rust controls the queue
[ ] Rust controls retries
[ ] Rust controls state
[ ] Rust creates checkpoints
[ ] Recovery works
[ ] Pause works
[ ] Resume works
[ ] Stop works
[ ] Extension communicates with Rust
[ ] Errors are structured
[ ] Indeterminate submissions are protected
[ ] No duplicate submissions occur during tested recovery scenarios
[ ] Chromium works
[ ] Firefox works
[ ] Security review passes
[ ] Tests pass
[ ] Documentation is updated
[ ] Old TypeScript automation is no longer required
```

---

# 25. Coding Agent Rules

While implementing this migration:

### DO

* Inspect the existing code before changing it.
* Reuse working concepts from the TypeScript implementation.
* Make one change at a time.
* Run tests/checks after meaningful changes.
* Keep commits small and understandable.
* Report exactly what changed.
* Report errors instead of hiding them.
* Stop and investigate indeterminate submissions.
* Prefer simple solutions.

### DO NOT

* Delete the old implementation prematurely.
* Rewrite the entire project in one operation.
* Add unnecessary frameworks.
* Duplicate queue logic.
* Duplicate retry logic.
* Put portal automation into Rust.
* Put job orchestration into Python.
* Put business logic into the extension.
* Automatically retry unknown submissions.
* Add arbitrary `sleep()` calls to hide synchronization problems.
* Hardcode trainee names.
* Store credentials in source code.
* Continue to the next migration stage when the current stage is broken.

---

# 26. Implementation Workflow

For every stage, follow:

```text
READ
 ↓
UNDERSTAND
 ↓
PLAN
 ↓
IMPLEMENT
 ↓
RUN CHECKS
 ↓
TEST
 ↓
FIX
 ↓
VERIFY
 ↓
DOCUMENT
 ↓
NEXT STAGE
```

The coding agent should **not skip ahead**.

The immediate priority is:

```text
1. Verify repository
2. Finish Rust ↔ Python protocol
3. Get ONE trainee working end-to-end
```

Only after those three are verified should the agent proceed to the Rust queue and state system.
