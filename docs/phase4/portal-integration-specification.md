# DSSP Portal Integration Specification

Status: **not started**

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

| Item                                        | Value |
| ------------------------------------------- | ----- |
| Portal origin                               |       |
| Trainee list URL pattern                    |       |
| Trainee detail URL pattern                  |       |
| Training form URL pattern                   |       |
| Signal that identifies a portal page        |       |
| Signal that identifies a logged-out session |       |

## 2. Trainee list

| Item                    | Selector | Notes |
| ----------------------- | -------- | ----- |
| List container          |          |       |
| Trainee row             |          |       |
| Trainee ID within row   |          |       |
| Trainee name within row |          |       |
| Link to trainee detail  |          |       |
| Empty-list indicator    |          |       |

## 3. Pagination and navigation

| Item                  | Selector | Notes                           |
| --------------------- | -------- | ------------------------------- |
| Next page control     |          |                                 |
| Page indicator        |          |                                 |
| Total count indicator |          |                                 |
| Behaviour             |          | Full reload or in-place update? |

## 4. Training form

| Field          | Selector | Control type | Value format |
| -------------- | -------- | ------------ | ------------ |
| Date           |          |              |              |
| Instructor     |          |              |              |
| Training type  |          |              |              |
| Submit control |          |              |              |

For every `select`, record whether options are static or loaded asynchronously,
and whether the value is an ID or a display string.

## 5. Result detection

| Outcome              | Selector or signal | Notes |
| -------------------- | ------------------ | ----- |
| Success confirmation |                    |       |
| Validation error     |                    |       |
| Duplicate record     |                    |       |
| Session expired      |                    |       |
| Server error         |                    |       |

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
