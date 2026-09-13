# DSSP Portal Integration Specification

Status: **training form mapped**

This document must be completed before `DSSPPortalAdapter` is implemented. Do
not write selectors into code that are not recorded here first. Every entry
needs a primary selector plus the evidence it was observed on a real page.

## Selector rules

Prefer, in order: stable `id`, `name`, `data-*` attribute, accessible label or
role, stable class, semantic relationship to a labelled element. Do not use
positional selectors such as `nth-child` chains.

Record every selector in this table and mirror it into
`src/core/infrastructure/portal/` only. No selector may appear anywhere else in
the codebase.

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
| Trainee row             | `table.table-checkable tbody tr` | Current page only                                        |
| Trainee ID within row   | `a[href*="TraineeId"]`           | Extract `TraineeId` query parameter from the delete link |
| Trainee name within row | Third `td`                       | Column index 2                                           |
| Link to trainee detail  |                                  | Delete URL is retained as the current row URL            |
| Empty-list indicator    |                                  |                                                          |

## 3. Pagination and navigation

| Item                  | Selector | Notes                           |
| --------------------- | -------- | ------------------------------- |
| Next page control     |          |                                 |
| Page indicator        |          |                                 |
| Total count indicator |          |                                 |
| Behaviour             |          | Full reload or in-place update? |

TODO: `getTrainees` currently reads only the visible page. Decide whether to
automate pagination or increase the portal's records-per-page setting.

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
| Page loading indicator     |                    |                   |
| Form submission spinner    |                    |                   |
| Slowest observed operation |                    |                   |

## 7. Open questions

Record anything ambiguous here rather than guessing in code.
