import {
  BRIDGE_SOURCE,
  isBridgeEvent,
  isBridgeResponse,
  type BridgeCommand,
  type BridgeRequest,
  type PortalEvent,
} from "./BridgeProtocol";
import { TimeoutError } from "../../shared/errors";

/**
 * ISOLATED-world client for the MAIN-world half of the content script.
 *
 * Correlates each request with its reply by id, since postMessage is a
 * broadcast channel with no request/response semantics of its own.
 */
export class BridgeClient {
  private sequence = 0;

  private readonly pending = new Map<
    string,
    { resolve: (value: unknown) => void; timer: ReturnType<typeof setTimeout> }
  >();

  private readonly eventHandlers = new Set<(event: PortalEvent) => void>();

  private started = false;

  private readonly window: Window;
  private readonly defaultTimeoutMs: number;

  constructor(target: Window, defaultTimeoutMs = 5000) {
    this.window = target;
    this.defaultTimeoutMs = defaultTimeoutMs;
  }

  /** Idempotent: safe to call on every command. */
  start(): void {
    if (this.started) return;
    this.started = true;

    this.window.addEventListener("message", (event: MessageEvent) => {
      if (event.source !== this.window) return;

      if (isBridgeResponse(event.data)) {
        this.settle(event.data.id, event.data.response);
        return;
      }

      if (isBridgeEvent(event.data)) {
        for (const handler of this.eventHandlers) {
          handler(event.data.event);
        }
      }
    });
  }

  /** Subscribe to unsolicited page events (alerts, submission responses). */
  onEvent(handler: (event: PortalEvent) => void): () => void {
    this.eventHandlers.add(handler);
    return () => this.eventHandlers.delete(handler);
  }

  /**
   * Send a command to the MAIN world and await its reply.
   *
   * Rejects with TimeoutError if MAIN never answers — which is what happens
   * when the MAIN script failed to inject, so the caller can tell "bridge is
   * down" apart from "command failed".
   */
  async send(
    command: BridgeCommand,
    timeoutMs = this.defaultTimeoutMs,
  ): Promise<unknown> {
    this.start();

    const id = `dssp-${++this.sequence}`;

    const message: BridgeRequest = {
      source: BRIDGE_SOURCE,
      direction: "request",
      id,
      command,
    };

    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new TimeoutError("MAIN world bridge", timeoutMs));
      }, timeoutMs);

      this.pending.set(id, { resolve, timer });

      this.window.postMessage(message, this.window.location.origin);
    });
  }

  /** True when the MAIN half is injected and answering. */
  async isReady(timeoutMs = 1000): Promise<boolean> {
    try {
      await this.send({ type: "BRIDGE_PING" }, timeoutMs);
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Suppress the page's dialogs for at most `ttlMs`.
   *
   * Only for the window around a submission this extension started: while
   * armed, `confirm()` answers Yes to everything, including the portal's own
   * destructive prompts. The deadline is the safety net — if this half dies
   * before disarming, the page recovers by itself.
   */
  async armDialogs(ttlMs: number): Promise<void> {
    await this.send({ type: "BRIDGE_ARM_DIALOGS", ttlMs });
  }

  /** Hand dialogs back to the user. Safe to call when already disarmed. */
  async disarmDialogs(): Promise<void> {
    await this.send({ type: "BRIDGE_DISARM_DIALOGS" });
  }

  /**
   * Run `operation` with dialogs suppressed, then always hand them back.
   *
   * `ttlMs` should exceed the operation's own timeout so the deadline only
   * fires when this half has genuinely stopped running.
   */
  async withDialogsArmed<T>(
    ttlMs: number,
    operation: () => Promise<T>,
  ): Promise<T> {
    await this.armDialogs(ttlMs);

    try {
      return await operation();
    } finally {
      // A failed disarm is not worth failing the operation over: the deadline
      // set above restores the page regardless.
      await this.disarmDialogs().catch(() => undefined);
    }
  }

  private settle(id: string, response: unknown): void {
    const entry = this.pending.get(id);
    if (!entry) return;

    clearTimeout(entry.timer);
    this.pending.delete(id);
    entry.resolve(response);
  }
}
