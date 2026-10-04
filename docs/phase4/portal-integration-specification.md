# DSSP Portal Integration Specification

Status: **training form mapped**

This document is the record of what was mapped on the real portal. Where the
implementation went beyond it, or where a section has not been observed at all,
the gap is recorded here rather than left implicit — a rule that is silently
ignored trains everyone to ignore the rest of the document. Every entry needs a
primary selector plus the evidence it was observed on a real page; an entry that
has not been observed says so.

## Selector rules

Prefer, in order: stable `id`, `name`, `data-*` attribute, accessible label or
role, stable class, semantic relationship to a labelled element. Do not use
positional selectors such as `nth-child` chains.

Record every selector in this table and mirror it into
`src/core/infrastructure/portal/` (legacy TypeScript) and `python/app/portal/`
(the port) only. No selector may appear anywhere else in the codebase.

## 1. Portal identity

| Item                                        | Value                                            |
| ------------------------------------------- | ------------------------------------------------ |
| Portal origin                               | `https://dssp.frsc.gov.ng`                       |
| Trainee list URL pattern                    | `/Trainee`                                       |
| Trainee detail URL pattern                  | `/Trainee/*`                                     |
| Training form URL pattern                   | `/Trainee/TrainingLog?TraineeId={id}`            |
| Training submission endpoint                | AJAX POST `/Trainee/LogTraining`                  |
| Authenticated-page marker                   | `form#logoutForm`, action `/Account/LogOff?area=`; legacy Logout markers retained |
| Signal that identifies a portal page        | DSSP origin plus a `/Trainee` path               |
| Signal that identifies a logged-out session | `#loginForm` or `form[action*="/Account/Login"]` |

## 2. Trainee list

| Item                    | Selector                         | Notes                                                    |
| ----------------------- | -------------------------------- | -------------------------------------------------------- |
| List container          | `table.table-checkable tbody`    | Server-rendered table body                               |
| Trainee row             | `table.table-checkable tbody tr` | The whole list, via the `pgsize` request in §3            |
| Trainee ID within row   | Training-log link containing `TraineeId=`; other ID links are fallback only | Extract numeric `TraineeId` from the training link preferentially |
| Trainee name within row | Third `td`                       | Column index 2 — see the exception below                 |
| Training-history link | `a[href*="/Trainee/TrainingLog?"][href*="TraineeId="]` | Preferred for `profile_url` even when a details link appears first; legacy slash form retained as an alternative |
| Empty-list indicator    | —                                | none: zero parsed rows and "the portal has no trainees" are the same value, so an unrendered page reads as an empty list |

Two behaviours of the row read are recorded here because they fail silently:

- **A row the parser cannot identify is dropped, not reported.** `get_trainees`
  skips any row whose link yields no numeric `TraineeId`
  (`python/app/portal/parsing.py:88`). A changed href shape therefore removes a
  trainee from the list entirely, and the batch reports `TRAINEE_NOT_FOUND` for
  someone who is on the portal.
- **The name is read positionally** (`cells[2]`), which the selector rules above
  forbid. It is recorded rather than used quietly: the row offers no stable
  per-cell hook, so the read breaks if the portal inserts a column.
  `_cell_text` (`parsing.py:66`) returns an empty string for a missing index
  rather than raising, so a shifted table yields a blank name — which surfaces as
  `TRAINEE_NOT_FOUND` on a name match, not as a crash.

## 3. Pagination and navigation

| Item                  | Selector | Notes                           |
| --------------------- | -------- | ------------------------------- |
| Next page control     | —        | not used; see decision below    |
| Page indicator        | —        | not used                        |
| Total count indicator | —        | taken from the response, not a selector: `list_trainees` returns `count` for the rows it parsed, while the portal's own page displays its total |
| Behaviour             | —        | one authenticated GET of the list URL; the worker does not click through pages |

**Decision (taken in code; recorded here at Stage 38).** Pagination is avoided
rather than implemented. `TRAINEE_LIST_URL` requests
`/Trainee?pgsize=10000&page=1&keywords=` (`python/app/portal/constants.py:25`),
so one request is expected to return the whole list.

The failure this carries is silent: if the portal caps `pgsize`, the list comes
back **truncated without an error**, and a trainee who plainly exists is reported
as `TRAINEE_NOT_FOUND`. The check is to compare the parsed `count` against the
total the portal displays on the same page — a mismatch is truncation. Capturing
both counts is a required output of the Stage 10 read-only run.

## 4. Training form

| Field          | Selector                                                                                                                           | Control type   | Value format                                        |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------- | -------------- | --------------------------------------------------- |
| Date           | `#TrainingDate`, `[name="TrainingDate"]`; associated `Training Date` label fallback                                                | `input`        | Strict `YYYY-MM-DD`                                 |
| Instructor     | `select#Instructor`, `select[name="Instructor"]`; MVC `InstructorId`/`InstructorID` variants and associated label fallback         | `select`       | Exact live option `value`; label retained unchanged |
| Training type  | `select#TrainingOptionId`, `select[name="TrainingOptionId"]`; existing `TrainingType`/`TrainingTypeId`/`TrainingTypeID` and label alternatives retained | `select` | Exact live option `value`; label retained unchanged |
| Submit control | External “Log Training” button calling `onTrainingLogSubmit()` | `button[type="button"]` | Worker reproduces AJAX; it does not click or submit the form |
| Trainee identity | `input#TraineeId[name="TraineeId"]` inside `form#frmtraininglog` | hidden input | One enabled, correctly owned control; value must match the resolved trainee ID |

The operator verified that `/Trainee/TrainingLog?TraineeId=...` contains
`form#frmtraininglog` without method/action, with `TrainingDate`, `InstructorId`,
and `TrainingOptionId`. Required-control searches now stay inside this form and
reject controls owned by another form. The reported AJAX contract is POST
`/Trainee/LogTraining`, with jQuery `data: { LogDetails: logdetails }` containing
`TraineeId`, `TrainingDate`, `InstructorId`, `TrainingOptionId`, and an empty
`FinalAssessments` array for ordinary sessions. `dataType: "json"` describes the
response, not a JSON request body. The JavaScript reload on success is not an
HTTP redirect.

The operator subsequently supplied the actual form and script, resolving the
initially mislabeled local HTML evidence. The Python implementation now validates
the final form URL and hidden trainee ID against the resolved record, then sends
exactly these ordinary-session fields as `application/x-www-form-urlencoded`:

```text
LogDetails[TraineeId]
LogDetails[TrainingDate]
LogDetails[InstructorId]
LogDetails[TrainingOptionId]
```

The brackets are percent-encoded on the wire. jQuery omits empty arrays, so
`FinalAssessments: []` adds no encoded fields. The worker does not serialize the
hidden assessment controls, unrelated inputs, or the separate logout token.
The form's missing action/method and external button do not affect this contract.

The captured JavaScript branches on training option `4` for final assessments.
That option, labels identifying final assessments, and nonempty assessment
payloads are blocked before POST; the distinct assessment flow is not supported.
POST redirects are not followed, so 307/308 cannot replay a training submission.
Login redirects report expiry; other 3xx responses remain unconfirmed.

Only synthetic fixtures were saved from this structure; no personal data or
real token from the supplied HTML was copied into code. The ordinary AJAX flow
is offline-verified, **not live-verified**; Stages 15/24 remain blocked.

The option set is never hardcoded. The extension scrapes every enabled,
non-placeholder option from the live form DOM, retaining both `option.value`
and the exact display label. When the trainee list is open, it performs an
authenticated GET of a real training form and scrapes the returned HTML.

## 5. Result detection

The table below describes the **Python migration worker**, including the local
`IsSuccessful` / HTTP-status correction following `d8b0588`.
These are fixture-verified rules, not proof of the real portal's response format.
The legacy TypeScript adapter still contains the broader text/empty-response/
redirect heuristics; this Python-only fix did not change that implementation.

| Outcome              | Selector or signal                                                                                            | Notes                     |
| -------------------- | ------------------------------------------------------------------------------------------------------------- | ------------------------- |
| Success confirmation | HTTP 2xx and boolean JSON `IsSuccessful: true` (`success`/`Success` retained), after login, rejection, and duplicate checks | Client-side contract supplied by operator; live POST response still missing |
| Validation error | `.validation-summary-errors`, `.field-validation-error`, recognized JSON success flag `false`, or HTTP `4xx` | Returned as rejected before duplicate-text checks |
| Duplicate record     | Response text containing `duplicate`, `already logged`, `already recorded`, or `already exists`               | Returned as duplicate     |
| Session expired      | Login URL or login form selector                                                                              | Stops the batch           |
| Server error         | HTTP `5xx`                                                                                                    | Returned as rejected      |

A submission counts as successful only when the explicit confirmation signal
appears. A clicked button, generic success text (including negated wording), an
empty HTTP 200/204 response, or a redirect without confirmation is not success.
Absent another recognized outcome, the worker returns `indeterminate`, which
stops automatic resubmission.

Before using the fix on the real portal, establish its actual success-response
format from private evidence tied to a verified successful record. If it uses
HTML, support only a verified confirmation signal and add regression tests;
otherwise leave that response `indeterminate`. Do not replay an unconfirmed
submission to discover the response format.

## 6. Loading and timing

| Item                       | Selector or signal | Observed duration |
| -------------------------- | ------------------ | ----------------- |
| Page loading indicator     | none — the worker waits on Playwright navigations, not on a spinner selector | not observed |
| Form submission spinner    | Captured UI script uses `body.loadingModal`; the worker reads the AJAX response directly and does not wait on a spinner | duration not observed |
| Slowest observed operation | n/a | not observed; bounded by `DSSP_NAV_TIMEOUT_MS` (30 s) per navigation and `DSSP_LOGIN_TIMEOUT_MS` (300 s) for the manual login |

Stage 10's read-only session and retrieval run passed on 2026-09-28, as recorded
in the migration runbook. Submission gates 15/24 remain blocked, and no measured
loading/spinner timings are recorded here. The "none / not observed" entries
are not evidence that the real submission flow has no spinner requirement.

## 7. Open questions

Record anything ambiguous here rather than guessing in code.

- The supplied HTML establishes `IsSuccessful` / `Message` handling, but no live
  POST response has been captured. Verify status, response body, and record
  creation before passing the live gates. Do not replay an existing session.
- Final-assessment submission remains unsupported until its distinct validation
  and payload flow is explicitly implemented and tested.
