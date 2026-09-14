# DSSP-Bot

Rebuild of the DSSP Auto Logger: Rust (coordinator/state/retry) + Python (Playwright worker) + browser extension (UI).

## Status
🚧 In migration from the legacy TypeScript extension. See `docs/` and `migration/python-rust` branch.

## Structure
- `extension/` — legacy TS extension (being phased out)
- `python/` — Playwright automation worker
- `rust/` — job queue, state machine, retry logic
- `docs/` — documentation and checklists
