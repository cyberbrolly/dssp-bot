/**
 * Wire protocol between the two content-script worlds.
 *
 * The ISOLATED world owns `chrome.*` but cannot see the page's real `window`,
 * so an `alert` override there is invisible to the portal's own code. The MAIN
 * world shares the page's globals but has no extension APIs. Neither half can
 * do the job alone, so they talk over `window.postMessage`.
 *
 * `postMessage` is shared with the page: anything here is readable and
 * forgeable by portal scripts. Keep payloads free of anything sensitive, and
 * validate every inbound frame (see `isBridgeMessage`).
 */

/** Marks a frame as ours. The page can forge it, so this is routing, not trust. */
export const BRIDGE_SOURCE = "dssp-bridge";

/**
 * Commands the MAIN world answers. Declared here so both halves agree on the
 * names without either importing the other.
 *
 * `BRIDGE_ARM_DIALOGS` enables dialog suppression for `ttlMs` and nothing
 * longer. Suppression answers every `confirm()` with Yes, so it must cover only
 * the window around a submission the extension itself started — never the whole
 * time the page is open.
 */
export type BridgeCommand =
  | { type: "BRIDGE_PING" }
  | { type: "BRIDGE_DRAIN_ALERTS" }
  | { type: "BRIDGE_DRAIN_RESPONSES" }
  | { type: "BRIDGE_ARM_DIALOGS"; ttlMs: number }
  | { type: "BRIDGE_DISARM_DIALOGS" };

/** Sent ISOLATED -> MAIN. `id` correlates the reply. */
export interface BridgeRequest {
  source: typeof BRIDGE_SOURCE;
  direction: "request";
  id: string;
  /**
   * A BridgeCommand, structured-cloned across the boundary. Typed `unknown`
   * because the page can post this shape too: the MAIN half must validate it
   * rather than trust it.
   */
  command: unknown;
}

/** Sent MAIN -> ISOLATED, echoing the request `id`. */
export interface BridgeResponse {
  source: typeof BRIDGE_SOURCE;
  direction: "response";
  id: string;
  /** A PortalCommandResponse. */
  response: unknown;
}

/**
 * Sent MAIN -> ISOLATED without a request: something happened on the page that
 * the automation needs to know about, such as an alert firing or a submission
 * XHR completing.
 */
export interface BridgeEvent {
  source: typeof BRIDGE_SOURCE;
  direction: "event";
  event: PortalEvent;
}

export type PortalEvent =
  | { type: "ALERT"; message: string; at: string }
  | {
      type: "SUBMISSION_RESPONSE";
      status: number;
      body: string;
      url: string;
      at: string;
    };

export type BridgeMessage = BridgeRequest | BridgeResponse | BridgeEvent;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function isBridgeMessage(value: unknown): value is BridgeMessage {
  if (!isRecord(value) || value["source"] !== BRIDGE_SOURCE) {
    return false;
  }

  const direction = value["direction"];

  if (direction === "request" || direction === "response") {
    return typeof value["id"] === "string";
  }

  return direction === "event" && isRecord(value["event"]);
}

export function isBridgeResponse(value: unknown): value is BridgeResponse {
  return isBridgeMessage(value) && value.direction === "response";
}

export function isBridgeRequest(value: unknown): value is BridgeRequest {
  return isBridgeMessage(value) && value.direction === "request";
}

export function isBridgeEvent(value: unknown): value is BridgeEvent {
  return isBridgeMessage(value) && value.direction === "event";
}
