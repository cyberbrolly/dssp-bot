# ADR 0001 — Support Firefox alongside Chromium

Date: 2026-09-07

Status: **proposed**

## Context

DSSP-bot is a Manifest V3 extension currently targeting Chromium only.
`README.md:79` states "Primary target is Chromium (Brave, Chrome, Edge)", and
`CHANGELOG.md:47` records that `webextension-polyfill` was removed as declared
but never imported.

The new product direction calls for a persistent Firefox session. Firefox
support is now a requirement, not a nice-to-have.

## Investigation

An initial reading of the codebase concluded that `content_scripts[].world:
"MAIN"` was a Chromium-only field and therefore a design-level blocker, since
the dialog-suppression and XHR-capture bridge depends on it. **That conclusion
was wrong.** MDN browser-compat-data:

| Manifest key                 | Chrome | Firefox   | Safari |
| ---------------------------- | ------ | --------- | ------ |
| `content_scripts[].world`    | 111    | **128**   | 18     |
| `background.service_worker`  | 88     | **false** | 15.4   |
| `background.scripts`         | MV2 only | 48      | 14     |
| `background.type: "module"`  | 92     | 112       | 16.4   |

`world: "MAIN"` is a standard WebExtensions key, supported on Firefox 128+. The
MAIN-world script, the `postMessage` bridge, and `DialogGate` all port unchanged.
No `wrappedJSObject` or `exportFunction` rewrite is needed.

The one genuine manifest blocker is `background.service_worker`, which Firefox
does not implement at any version.

A second finding materially lowers the risk. `submitTrainingForm`
(`DSSPPortalAdapter.ts:967`) submits by `fetch`, not by clicking the page's
submit control. The portal's jQuery submit handler never runs, so the extension's
own submission provokes no `alert()`. The dialog gate is defensive only — it
guards against a dialog the extension does not currently trigger.

That makes one existing detail a latent fragility on any browser:
`withDialogsArmed` awaits `armDialogs()` *outside* its `try` block
(`BridgeClient.ts:135`), so if the MAIN world is not answering, `armDialogs`
rejects with `TimeoutError` and the submission never runs. A defensive guard can
currently block the operation it is meant to protect.

## Decision

Support Firefox and Chromium from one codebase.

1. **Per-browser manifest.** `createManifest` takes a target. Chromium keeps
   `background.service_worker`; Firefox emits `background.scripts` plus
   `browser_specific_settings.gecko.id` and `strict_min_version: "128"`, the
   floor for `world: "MAIN"`.

2. **Keep `world: "MAIN"`.** Supported on both targets. The bridge, `DialogGate`,
   and `main-world.ts` are unchanged.

3. **`FirefoxBrowserAdapter` beside `ChromiumBrowserAdapter`.** `BrowserAdapter`
   is already the right seam. Firefox's `chrome.*` is a callback-style alias that
   does not return promises, so `ChromiumStorageArea` and
   `ChromiumRuntime.sendMessage` would resolve `undefined` there. The Firefox
   adapter uses the promise-based `browser.*` namespace.

4. **Make dialog arming non-fatal.** Move `armDialogs()` inside the `try` and
   treat failure as "not armed" rather than as operation failure. Since the
   extension's own submission provokes no dialog, a down bridge must not block a
   submission. Correct on Chromium too.

5. **Hoist the hardcoded origin.** `https://dssp.frsc.gov.ng` is hardcoded in
   `ChromiumBrowserAdapter.ts:53` and `RemotePortalAdapter.ts:51`, duplicating
   what `VITE_PORTAL_MATCHES` controls. Pointing the extension at a staging
   origin does not currently fully redirect it.

## Consequences

Good:

- One codebase, two targets, no polyfill dependency.
- The safety-critical dialog logic is untouched, so its behaviour need not be
  re-proven from scratch.
- Fixing the arming order removes a failure mode that exists on Chromium today.

Costs:

- Two build outputs and two manifests to keep in step.
- Firefox 128+ only. Acceptable: 128 is the ESR baseline.
- `BrowserAdapter` gains a second implementation to maintain.

Risks:

- Firefox MV3 background scripts are event pages with different termination
  behaviour than service workers. The existing checkpoint machinery already
  assumes the worker can die mid-batch, which is the right shape, but the
  restart path needs verifying on Firefox specifically.
- MV3 content-script `fetch` is subject to page CORS on both browsers. The
  submission POST is same-origin, so this should not bite — but it is untested
  on Firefox.

## Alternatives rejected

**Chromium only, treat "Firefox" as shorthand for a persistent session.** The
requirement was explicit.

**Port to Firefox only.** Discards working Chromium support for no gain.

**Replace `world: "MAIN"` with `wrappedJSObject`/`exportFunction`.** Was
premised on my incorrect reading of the compat data. It would mean rewriting the
safety-critical dialog path for no reason.
