/**
 * MAIN-world half of the content script.
 *
 * Runs in the page's own JavaScript context, so `window.alert` here is the
 * function the portal actually calls and `XMLHttpRequest` is the constructor it
 * actually uses. Neither is reachable from the ISOLATED world, which is the
 * whole reason this file exists.
 *
 * No `chrome.*` API is available here. Everything that needs one goes through
 * the postMessage bridge to content-script.ts. Imports must stay free of
 * extension APIs — BridgeProtocol is types and pure predicates only.
 *
 * Injected at document_start, but @crxjs loads this chunk through a dynamic
 * import, so the patches land a tick later rather than truly first. That is
 * fine for this portal: its scripts call `alert` from jQuery event handlers at
 * submit time, long after load. It would not be safe against a page that
 * captured `window.alert` into a local during its own first script.
 */

import {
  BRIDGE_SOURCE,
  isBridgeRequest,
  type BridgeResponse,
  type PortalEvent,
} from "../core/infrastructure/portal/BridgeProtocol";
import { DialogGate } from "../core/infrastructure/portal/DialogGate";

/** Alerts seen since load, newest last. Drained by the ISOLATED world. */
const alertLog: Array<{ message: string; at: string }> = [];

/** Submission responses captured from XHR, newest last. */
const responseLog: Array<{
  status: number;
  body: string;
  url: string;
  at: string;
}> = [];

const MAX_LOG = 50;

function emit(event: PortalEvent): void {
  window.postMessage(
    { source: BRIDGE_SOURCE, direction: "event", event },
    window.location.origin,
  );
}

function reply(id: string, response: unknown): void {
  const message: BridgeResponse = {
    source: BRIDGE_SOURCE,
    direction: "response",
    id,
    response,
  };

  window.postMessage(message, window.location.origin);
}

/** Decides whether dialogs are currently suppressed. Starts disarmed. */
const dialogs = new DialogGate();

/**
 * Render a dialog argument as text.
 *
 * The portal passes whatever it likes to alert(). Since the message text is the
 * only success/failure signal available, an object must not collapse to
 * "[object Object]" — that would discard the outcome of a submission.
 */
function toText(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;

  if (
    typeof value === "number" ||
    typeof value === "boolean" ||
    typeof value === "bigint"
  ) {
    return String(value);
  }

  try {
    return JSON.stringify(value) ?? "";
  } catch {
    return "[unserialisable dialog message]";
  }
}

function recordAlert(text: string, at: string): void {
  alertLog.push({ message: text, at });
  if (alertLog.length > MAX_LOG) alertLog.shift();

  emit({ type: "ALERT", message: text, at });
}

/**
 * Swallow the portal's modal dialogs and record them instead — but only while
 * armed.
 *
 * A native alert blocks the page until dismissed, and nothing in an extension
 * can click it. Left alone it would stall every batch on the first submission.
 * The text is the portal's only success/failure signal, so it is captured
 * rather than discarded.
 *
 * Suppression is deliberately NOT the default. `confirm()` answering Yes
 * unconditionally would auto-accept the portal's own destructive prompts for an
 * administrator browsing by hand, and a swallowed `alert()` would hide the
 * portal's feedback from them. So the wrappers are installed at document_start
 * (to be in place before the portal's handlers run) but pass straight through
 * to the originals until the ISOLATED half arms them around a submission it
 * started.
 *
 * Arming carries a deadline rather than a flag: if the ISOLATED half is torn
 * down mid-batch — navigation, a killed service worker, a thrown handler — the
 * page returns to normal on its own instead of staying suppressed for as long
 * as the tab is open.
 */
function interceptDialogs(): void {
  /* eslint-disable @typescript-eslint/unbound-method --
   * Captured off `window` only to re-invoke via Reflect.apply with the original
   * `this`, which is how these are wrapped without losing the originals. */
  const originalAlert = window.alert;
  const originalConfirm = window.confirm;
  /* eslint-enable @typescript-eslint/unbound-method */

  // Every dialog is recorded either way: the text is useful evidence even when
  // the extension is only observing. Only the suppression is conditional.
  window.alert = (message?: unknown): void => {
    recordAlert(toText(message), new Date().toISOString());

    // Not armed: this is the administrator's own browsing, so the portal must
    // behave exactly as it would without the extension installed.
    if (!dialogs.armed) {
      Reflect.apply(originalAlert, window, [message]);
    }
  };

  window.confirm = (message?: unknown): boolean => {
    recordAlert(toText(message), new Date().toISOString());

    // Answering true is only safe for a prompt the extension itself provoked.
    // Outside that window the portal's own "are you sure?" must reach a human.
    if (!dialogs.armed) {
      return Reflect.apply(originalConfirm, window, [message]) as boolean;
    }

    return true;
  };
}

/**
 * Record responses to portal POSTs.
 *
 * The portal reports outcomes through an alert, which is easy to miss and
 * ambiguous. The underlying HTTP response is the more reliable signal, and
 * capturing it is only possible from this world.
 */
function interceptXhr(): void {
  /* eslint-disable @typescript-eslint/unbound-method --
   * These are read off the prototype only to re-invoke them via Reflect.apply
   * with the original `this`, which is the standard way to wrap a method. */
  const originalOpen = XMLHttpRequest.prototype.open;
  const originalSend = XMLHttpRequest.prototype.send;
  /* eslint-enable @typescript-eslint/unbound-method */

  // Request metadata kept beside the object rather than on it, so the page
  // cannot see the properties and nothing leaks if a request is abandoned.
  const tracked = new WeakMap<XMLHttpRequest, { url: string; post: boolean }>();

  // open() is overloaded (2-arg and 5-arg forms), so the wrapper takes a rest
  // parameter and forwards whatever the caller actually passed.
  XMLHttpRequest.prototype.open = function (
    this: XMLHttpRequest,
    method: string,
    url: string | URL,
    ...rest: [boolean?, (string | null)?, (string | null)?]
  ): void {
    tracked.set(this, {
      url: typeof url === "string" ? url : url.href,
      post: method.toUpperCase() === "POST",
    });

    Reflect.apply(originalOpen, this, [method, url, ...rest]);
  };

  XMLHttpRequest.prototype.send = function (
    this: XMLHttpRequest,
    ...args: Parameters<typeof originalSend>
  ): void {
    const meta = tracked.get(this);

    if (meta?.post) {
      this.addEventListener("loadend", () => {
        // responseText throws for non-text responseTypes; those are not the
        // form posts being watched for, so treat them as empty.
        let body: string;

        try {
          body = this.responseText;
        } catch {
          body = "";
        }

        const entry = {
          status: this.status,
          body,
          url: meta.url,
          at: new Date().toISOString(),
        };

        responseLog.push(entry);
        if (responseLog.length > MAX_LOG) responseLog.shift();

        emit({ type: "SUBMISSION_RESPONSE", ...entry });
      });
    }

    Reflect.apply(originalSend, this, args);
  };
}

/**
 * Bridge commands this world can answer.
 *
 * Only the page-global dialog and XHR interception lives here. Form discovery
 * and submission are handled by the isolated-world portal adapter.
 */
function handle(command: unknown): unknown {
  const type =
    typeof command === "object" &&
    command !== null &&
    "type" in command &&
    typeof command.type === "string"
      ? command.type
      : "";

  switch (type) {
    case "BRIDGE_PING":
      return {
        success: true,
        data: { ready: true, url: window.location.href },
      };

    case "BRIDGE_DRAIN_ALERTS": {
      const drained = alertLog.splice(0, alertLog.length);
      return { success: true, data: drained };
    }

    case "BRIDGE_DRAIN_RESPONSES": {
      const drained = responseLog.splice(0, responseLog.length);
      return { success: true, data: drained };
    }

    case "BRIDGE_ARM_DIALOGS": {
      const ttlMs = dialogs.arm(
        typeof command === "object" && command !== null && "ttlMs" in command
          ? command.ttlMs
          : undefined,
      );

      if (ttlMs === 0) {
        return {
          success: false,
          error: "BRIDGE_ARM_DIALOGS requires a positive ttlMs.",
          code: "MISSING_DATA",
        };
      }

      return { success: true, data: { armedForMs: ttlMs } };
    }

    case "BRIDGE_DISARM_DIALOGS":
      dialogs.disarm();

      return { success: true, data: { armed: false } };

    default:
      return {
        success: false,
        error: `MAIN world cannot handle ${type || "unknown command"}`,
        code: "PORTAL_NOT_MAPPED",
      };
  }
}

function listen(): void {
  window.addEventListener("message", (event: MessageEvent) => {
    // Only same-window frames. The page can still forge these, so treat the
    // payload as untrusted input; it selects a handler and nothing more.
    if (event.source !== window) return;
    if (!isBridgeRequest(event.data)) return;

    try {
      reply(event.data.id, handle(event.data.command));
    } catch (error) {
      reply(event.data.id, {
        success: false,
        error: error instanceof Error ? error.message : String(error),
        code: "BRIDGE_HANDLER_FAILED",
      });
    }
  });
}

interceptDialogs();
interceptXhr();
listen();
