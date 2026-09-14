# Persistent Session Architecture

Status: **proposed** — supersedes the ad-hoc session hooks currently on
`PortalAdapter`.

DSSP-bot becomes a persistent batch-processing agent: the operator configures
the training details once, pastes a trainee queue, and the extension repeatedly
does the browser work, stopping for a human only when something looks off.

## 1. Target flow

```
Configure          instructor, training type, date -> validated once
      |
Load Trainee Logs  one scrape of the enrolled-trainees table
      |
Enter queue        comma/newline input -> trimmed, deduped, resolved
      |
Start batch        session initialised once
      |
   .--<-------------------------------------------.
   |                                              |
Prepare trainee -> Fill -> Validate -> Submit -> Verify
   |                                              |
   '--> anomaly? -> review gate -> continue/skip/stop
```

The loop reuses one training session. It reinitialises only when the session
becomes unusable, never on a schedule.

## 2. Layering

Unchanged from `README.md`; this document only sharpens the responsibilities.

| Layer             | Owns                                                        | Must not                          |
| ----------------- | ----------------------------------------------------------- | --------------------------------- |
| `BatchRunner`     | Input resolution, queue construction, date normalisation     | Touch the session or retry        |
| `AutomationEngine`| Queue, session lifecycle, retries, progress, review gates    | Know any selector or URL          |
| `PortalAdapter`   | The DSSP page: DOM, HTTP, form state                         | Decide batch policy               |

## 3. Why the current hooks fail

The three session hooks are declared optional on `PortalAdapter`:

```ts
initializeTrainingSession?: () => Promise<Result<void>>;
isTrainingSessionReady?: () => Promise<boolean>;
prepareTrainee?: (trainee: Trainee) => Promise<Result<void>>;
```

`RemotePortalAdapter` — the only adapter the service worker gives the engine —
implements none of them, and optionality means the compiler is satisfied.
`AutomationEngine.prepareSubmission` feature-detects, finds nothing, and falls
back to `openTrainee`, which records state without loading a form. The next
step, `fillTrainingForm`, hits its `!this.preparedForm` guard and returns
`MissingDataError` — non-recoverable, so no retry.

**Every trainee in every batch fails.** The 155 unit tests pass because
`DSSPPortalAdapter.test.ts` exercises `prepareTrainee` directly on the local
adapter, never across the remote hop.

### Decision 1 — the session hooks become required

Optionality was the defect. A required method forces every implementation to
answer for itself and turns this class of gap into a compile error.
`openTraineeLogs` is already required, so the interface is inconsistent today.

### Decision 2 — the wire protocol gains the missing arms

`PortalCommands.ts` has no arm for any session hook, and `content-script.ts` has
no case, so the commands could not cross the boundary even if implemented.

Add `PORTAL_INITIALIZE_SESSION`, `PORTAL_SESSION_READY`,
`PORTAL_PREPARE_TRAINEE`.

### Decision 3 — an exhaustiveness test guards the boundary

A unit test asserts that every `PortalAdapter` method has a `PortalCommand` arm,
and that every arm has a `content-script.ts` case. This is the regression guard
for the whole bug class, not just today's three methods.

## 4. Session lifecycle

### What a session is

Two things are currently conflated under "session ready":

| Concern              | Scope      | Changes when                        |
| -------------------- | ---------- | ----------------------------------- |
| Training config      | Per batch  | The operator reconfigures            |
| Prepared form document | Per trainee | The queue advances                 |

`isTrainingSessionReady` today only probes the second. The first is never
validated up front, so a wrong instructor id is discovered on trainee 1 —
after the batch has started.

### Decision 4 — validate the config once, before any record is written

`initializeTrainingSession` becomes:

1. Confirm the portal page is authenticated.
2. Load the form option sets (instructors, training types).
3. Resolve the configured instructor and training type against those sets.
4. Normalise the training date.

Failure here aborts before trainee 1. A systematic misconfiguration can no
longer burn a trainee to be discovered.

### Decision 5 — session init stops re-scraping the trainee list

Today `BatchRunner.start` calls `openTraineeLogs()` then `getTrainees()`.
`openTraineeLogs` internally calls `getTrainees()` and caches; `getTrainees()`
then consumes and nulls that cache. `initializeTrainingSession` calls **both
again** — a second `pgsize=10000` scrape — purely to take `trainees.data[0]` as
a seed for a throwaway `openTrainingForm`, whose document the first real
`prepareTrainee` immediately discards.

Session init no longer needs a trainee. It validates config against option
sets, nothing more. This removes one full scrape and one wasted form fetch per
batch.

### Decision 6 — session generation is explicit

A monotonic `sessionGeneration: number`, incremented on every initialise. The
engine records it on each `TrainingResult`, so a report shows which records
were written under which session, and a mid-batch reinitialisation is visible
rather than inferred.

### Reinitialisation

Checked per trainee, as today, because a session can lapse mid-batch:

```
isTrainingSessionReady() -> false
        -> initializeTrainingSession()   (generation++)
        -> raise SESSION_REINITIALIZED anomaly (informational)
```

`isPortalPage()` stays a per-trainee and per-retry check. The existing comment
at `AutomationEngine.ts:508` defends this correctly: a check that cannot catch a
mid-run lapse is worthless, and a lapsed session is why a retry would otherwise
keep failing.

## 5. Queue construction

`parseTraineeInput` already handles the input rules: comma and newline splitting,
trimming, blank removal, and case/whitespace-insensitive dedupe. Resolution
against the live trainee list stays in `BatchRunner`, deduped a second time by
resolved id — both passes are needed, since two spellings can resolve to one
trainee.

### Decision 7 — unmatched input is reported before the batch starts

Unresolved names already accumulate into `missing[]` and fail collectively.
Numeric ids that match nothing are currently queued against a synthetic
`Trainee` stub named `Trainee <id>`, which is guaranteed to fail at submit.

Both become a pre-flight report the operator sees **before** starting, not a
per-trainee failure discovered during the run. Nothing is queued that is already
known to be unresolvable.

## 6. Review gates

The first target is assisted automation: run unattended, halt for a human when
something looks off.

### Decision 8 — anomalies are a typed, closed set

```ts
export type AnomalyCode =
  | "TRAINEE_UNRESOLVED"
  | "DUPLICATE_RECORD"
  | "SUBMISSION_REJECTED"
  | "OUTCOME_INDETERMINATE"
  | "SESSION_REINITIALIZED"
  | "CONSECUTIVE_FAILURES";

export type ReviewDecision = "continue" | "skip" | "stop";

export interface Anomaly {
  code: AnomalyCode;
  traineeId: string;
  traineeName: string;
  message: string;
  detectedAt: string;
  /** Applied if the gate is never answered. */
  defaultAction: ReviewDecision;
}
```

### Decision 9 — the default action on an unanswered gate biases toward stopping

A gate that hangs forever is a worse failure than one that stops the batch. Each
anomaly carries the action applied when the gate times out.

| Anomaly                 | Trigger                                        | Default | Why that default                                              |
| ----------------------- | ---------------------------------------------- | ------- | ------------------------------------------------------------- |
| `OUTCOME_INDETERMINATE` | Submitted, confirmation never arrived           | `stop`  | A record may exist. Continuing risks a second one.            |
| `SUBMISSION_REJECTED`   | Portal rejected the record                      | `stop`  | Usually systemic; it will reject every remaining trainee.     |
| `CONSECUTIVE_FAILURES`  | `k` failures in a row (default 3)               | `stop`  | Something is wrong with the run, not the trainee.             |
| `DUPLICATE_RECORD`      | Portal reports already logged                   | `skip`  | Already recorded. Skipping is the safe, correct outcome.      |
| `TRAINEE_UNRESOLVED`    | Queued id matched no trainee row                | `skip`  | Nothing can be written; the rest of the queue is unaffected.  |
| `SESSION_REINITIALIZED` | Session lapsed and was rebuilt                  | `continue` | Informational. The new session is already validated.       |

`OUTCOME_INDETERMINATE` currently aborts the whole batch. As a gate it becomes
recoverable: the operator checks the portal by hand and answers `skip` (it was
written) or `continue` (it was not).

### Decision 10 — gates are checkpointed

`BatchCheckpoint.status` gains `"awaiting_review"`. The service worker can be
killed while a gate is open, and the operator may be away for minutes. On
restart the gate is restored rather than silently lost.

### State machine changes

| Change                        | Reason                                                                 |
| ----------------------------- | ---------------------------------------------------------------------- |
| Add `awaiting_review`         | Reachable from any active state; exits to `loading_trainee`/`stopped`/`complete` |
| Remove `opening_form`         | Already unreachable — the engine stopped calling `openTrainingForm`     |
| Keep `loading_trainee`        | Now covers prepare-and-load, which `prepareTrainee` performs as one step |

`AutomationEngine.transition` silently skips illegal transitions rather than
throwing, so the table only narrows what gets recorded. That stays: a bad
transition must not fail a batch that is writing real records.

## 7. Messages and UI

The popup already polls `GET_STATUS` every second, so a gate needs no push
channel.

| Message          | Direction        | Purpose                                  |
| ---------------- | ---------------- | ---------------------------------------- |
| `GET_STATUS`     | popup -> worker  | Extended with the pending `Anomaly`      |
| `RESOLVE_REVIEW` | popup -> worker  | Carries a `ReviewDecision`               |

## 8. Deliberately out of scope

- **Full autonomy.** Review gates are the point of this phase, not a stepping
  stone to be removed.
- **Parallel submission.** The portal is a shared government system and the
  three-phase commit contract assumes one in-flight write.
- **Pagination.** `pgsize=10000` sidesteps it. `docs/phase4` section 3 still
  records this as open.

## 9. Open questions

| Question                                                                     | Blocks              |
| ---------------------------------------------------------------------------- | ------------------- |
| Review-gate timeout. 10 minutes is a guess; the operator's real absence is not. | Gate implementation |
| Should `SUBMISSION_REJECTED` gate on every occurrence, or only the first?     | Gate rules          |
| `RemotePortalAdapter.COMMAND_TIMEOUT_MS` is 15s while two stacked 10s waits are reachable. A `PORTAL_UNAVAILABLE` on submit classifies as indeterminate. | Timeout budget      |
