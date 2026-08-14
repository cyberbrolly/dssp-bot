# DSSP Training Logger

## Overview

DSSP Training Logger is a Manifest V3 browser extension that reduces repetitive
manual work when recording trainee driving sessions on the DSSP portal.

The portal remains the source of truth. The extension operates through the
already-authenticated browser session and drives the same user-facing workflow
an administrator would use by hand.

## Current Status

Phase 4 — Portal integration.

- Phase 1 — Planning: complete
- Phase 2 — Foundation: complete
- Phase 3 — Browser layer: complete
- Phase 4 — Portal discovery and training form integration: in progress

The trainee list and Log New Training form are mapped. Instructor and training
type choices are scraped from the authenticated portal form, dates are
normalized to `YYYY-MM-DD`, and submissions reuse the form's real field names,
hidden values, action, and method.

## Architecture

Three layers, with dependencies pointing inward:

- `src/core/` — domain models, automation engine, and infrastructure
  abstractions. Contains no portal selectors.
- `src/core/infrastructure/portal/` — the `PortalAdapter` interface and its
  implementations. All portal knowledge belongs here.
- `src/background/`, `src/content/`, `src/popup/` — extension layer: service
  worker, content script, and popup UI.

The engine calls `PortalAdapter` methods and never touches the DOM. The
background worker holds the engine and reaches the page through
`RemotePortalAdapter`, which forwards typed commands to the content script.

## Setup

```bash
pnpm install
cp .env.example .env
```

Set `VITE_PORTAL_MATCHES` in `.env` to the portal origin, for example
`https://portal.example.com/*`. It drives both `content_scripts.matches` and
`host_permissions`, so the extension only ever runs on the portal. Multiple
patterns can be comma-separated. The default is a placeholder that matches
nothing real.

## Commands

| Command          | Purpose                            |
| ---------------- | ---------------------------------- |
| `pnpm dev`       | Vite dev server with extension HMR |
| `pnpm build`     | Typecheck and produce `dist/`      |
| `pnpm typecheck` | `tsc --noEmit`                     |
| `pnpm lint`      | ESLint (type-aware)                |
| `pnpm format`    | Prettier write                     |
| `pnpm test`      | Vitest unit tests                  |

## Loading in Brave

1. Run `pnpm build`.
2. Open `brave://extensions` and enable Developer mode.
3. Choose "Load unpacked" and select `dist/`.

## Security

The extension never collects or stores the portal password. The administrator
logs in normally and the extension works inside that session. Local storage
holds settings, execution history, and the last batch report only.

## Tech Stack

TypeScript, Vite, Manifest V3, pnpm, ESLint, Prettier, Vitest. Primary target is
Chromium (Brave, Chrome, Edge).

## License

Private
