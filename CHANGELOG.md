# Changelog

## v0.1.0

### Added

- Layered architecture: core domain, automation, and infrastructure separated
  from the extension layer
- `AutomationEngine` with working pause, resume, and stop controls
- `StateMachine` enforcing the documented transition table
- `BatchRunner` for sequential trainee processing
- `RetryPolicy` with exponential backoff, applied only to recoverable errors
- `TrainingResult` and `BatchReport` types with completion-report totals
- `TrainingLogger` producing structured per-operation log entries
- `BrowserAdapter` abstraction over the Chromium extension APIs
- `PortalAdapter` interface covering the full portal operation surface
- `RemotePortalAdapter` bridging the background worker to the content script
- `UnmappedPortalAdapter` failing loudly until portal discovery is complete
- Popup UI with session configuration, live progress, and batch controls
- ESLint, Prettier, and Vitest with 34 unit tests

### Changed

- TypeScript `strict` enabled; `manifest.ts`, `vite.config.ts`, and `tests` are
  now typechecked
- Host permissions and content-script matches scoped to `VITE_PORTAL_MATCHES`
  instead of `<all_urls>`
- `Storage` and `MessageBus` depend on `BrowserAdapter` rather than the `chrome`
  global
- `waitForSubmissionResult` returns a `SubmissionOutcome` that distinguishes
  confirmed, duplicate, and rejected submissions

### Fixed

- `pause()` set the state but did not halt the queue, so a paused batch kept
  processing every remaining trainee
- No `resume()` existed, making pause unrecoverable
- The concurrency guard covered only two states, allowing a second `run()` to
  start a parallel loop over the same queue
- Unhandled message types returned `undefined` and crashed the popup

### Removed

- Stale empty duplicates of `DOMObserver`, `Messaging`, `StorageManager`, and
  `SelectorResolver`
- `src/types/`, `src/constants/`, and the unused Vite template entry point
- `webextension-polyfill`, which was declared but never imported
- `.env` from version control
