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
| Training form URL pattern                   | `/Trainee/TrainingLog/TraineeId={id}`            |
| Signal that identifies a portal page        | DSSP origin plus a `/Trainee` path               |
| Signal that identifies a logged-out session | `#loginForm` or `form[action*="/Account/Login"]` |

## 2. Trainee list

| Item                    | Selector                         | Notes                                                    |
| ----------------------- | -------------------------------- | -------------------------------------------------------- |
| List container          | `table.table-checkable tbody`    | Server-rendered table body                               |
| Trainee row             | `table.table-checkable tbody tr` | The whole list, via the `pgsize` request in §3            |
| Trainee ID within row   | `a[href*="TraineeId"]`           | Extract `TraineeId` query parameter from the delete link |
| Trainee name within row | Third `td`                       | Column index 2 — see the exception below                 |
| Link to trainee detail  |                                  | Delete URL is retained as the current row URL            |
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
| Training type  | `select#TrainingType`, `select[name="TrainingType"]`; MVC `TrainingTypeId`/`TrainingTypeID` variants and associated label fallback | `select`       | Exact live option `value`; label retained unchanged |
| Submit control | Associated form's `button[type="submit"]` or `input[type="submit"]`                                                                | submit control | Form action/method and hidden fields retained       |

The option set is never hardcoded. The extension scrapes every enabled,
non-placeholder option from the live form DOM, retaining both `option.value`
and the exact display label. When the trainee list is open, it performs an
authenticated GET of a real training form and scrapes the returned HTML.

## 5. Result detection

| Outcome              | Selector or signal                                                                                            | Notes                     |
| -------------------- | ------------------------------------------------------------------------------------------------------------- | ------------------------- |
| Success confirmation | JSON `success: true`, explicit success text, empty `2xx` response, `204`, or redirect away from `TrainingLog` | Direct form POST response |
| Validation error     | `.validation-summary-errors`, `.field-validation-error`, JSON `success: false`, or HTTP `4xx`                 | Returned as rejected      |
| Duplicate record     | Response text containing `duplicate`, `already logged`, `already recorded`, or `already exists`               | Returned as duplicate     |
| Session expired      | Login URL or login form selector                                                                              | Stops the batch           |
| Server error         | HTTP `5xx`                                                                                                    | Returned as rejected      |

A submission counts as successful only when the success signal appears. A
clicked button is not confirmation.

## 6. Loading and timing

| Item                       | Selector or signal | Observed duration |
| -------------------------- | ------------------ | ----------------- |
| Page loading indicator     | none — the worker waits on Playwright navigations, not on a spinner selector | not observed |
| Form submission spinner    | none — the submission is a `fetch` whose response is read directly, so no spinner is awaited | not observed |
| Slowest observed operation | n/a | not observed; bounded by `DSSP_NAV_TIMEOUT_MS` (30 s) per navigation and `DSSP_LOGIN_TIMEOUT_MS` (300 s) for the manual login |

Nothing here has been observed against the real portal — the live run
(Stages 10/15/24) has not happened. Each row is recorded as "none / not observed"
rather than left blank so the absence is a fact someone can act on: if the real
portal does gate on a spinner, this is the section that says nobody has looked.

## 7. Open questions

Record anything ambiguous here rather than guessing in code.
