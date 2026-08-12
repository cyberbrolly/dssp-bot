import { describe, expect, it, vi } from "vitest";
import { BridgeClient } from "../../src/core/infrastructure/portal/BridgeClient";
import {
  BRIDGE_SOURCE,
  isBridgeMessage,
  isBridgeRequest,
} from "../../src/core/infrastructure/portal/BridgeProtocol";

/**
 * Stand-in for the window shared by both worlds. postMessage is a broadcast
 * channel with no request/response semantics, which is what makes the
 * correlation and spoofing cases below worth asserting.
 */
class FakeWindow {
  readonly posted: unknown[] = [];
  readonly location = { origin: "https://portal.test" };

  private readonly listeners: Array<(event: MessageEvent) => void> = [];

  addEventListener(_type: string, handler: (event: MessageEvent) => void) {
    this.listeners.push(handler);
  }

  postMessage(message: unknown, _origin: string) {
    this.posted.push(message);
  }

  /** Simulate a frame arriving; `source` defaults to this window. */
  deliver(data: unknown, source: unknown = this) {
    for (const handler of this.listeners) {
      handler({ data, source } as unknown as MessageEvent);
    }
  }

  lastRequestId(): string {
    const request = [...this.posted].reverse().find(isBridgeRequest);
    if (!request) throw new Error("no bridge request was posted");
    return request.id;
  }
}

function setup(timeoutMs = 50) {
  const fake = new FakeWindow();
  const bridge = new BridgeClient(fake as unknown as Window, timeoutMs);
  bridge.start();
  return { bridge, fake };
}

function response(id: string, payload: unknown) {
  return {
    source: BRIDGE_SOURCE,
    direction: "response" as const,
    id,
    response: payload,
  };
}

describe("BridgeClient", () => {
  it("resolves a command with its matching reply", async () => {
    const { bridge, fake } = setup();

    const pending = bridge.send({ type: "BRIDGE_PING" });
    fake.deliver(response(fake.lastRequestId(), { success: true }));

    await expect(pending).resolves.toEqual({ success: true });
  });

  it("pairs concurrent commands with their own replies", async () => {
    const { bridge, fake } = setup();

    const first = bridge.send({ type: "BRIDGE_DRAIN_ALERTS" });
    const firstId = fake.lastRequestId();

    const second = bridge.send({ type: "BRIDGE_DRAIN_RESPONSES" });
    const secondId = fake.lastRequestId();

    expect(firstId).not.toBe(secondId);

    // Answered out of order: ids, not arrival order, decide the pairing.
    fake.deliver(response(secondId, "second"));
    fake.deliver(response(firstId, "first"));

    await expect(first).resolves.toBe("first");
    await expect(second).resolves.toBe("second");
  });

  it("times out when the MAIN world never answers", async () => {
    const { bridge } = setup(20);

    // This is what a failed MAIN-world injection looks like from here.
    await expect(bridge.send({ type: "BRIDGE_PING" })).rejects.toThrow(
      /Timed out/,
    );
  });

  it("reports not ready when the bridge is silent", async () => {
    const { bridge } = setup(20);

    await expect(bridge.isReady(20)).resolves.toBe(false);
  });

  it("ignores frames from another window", async () => {
    const { bridge, fake } = setup(20);

    const pending = bridge.send({ type: "BRIDGE_PING" });

    // An iframe or opener must not be able to satisfy our request.
    fake.deliver(response(fake.lastRequestId(), { success: true }), {
      name: "other-window",
    });

    await expect(pending).rejects.toThrow(/Timed out/);
  });

  it("ignores frames without the bridge marker", async () => {
    const { bridge, fake } = setup(20);

    const pending = bridge.send({ type: "BRIDGE_PING" });
    const id = fake.lastRequestId();

    fake.deliver({ direction: "response", id, response: "unmarked" });
    fake.deliver({ source: "other-extension", direction: "response", id });

    await expect(pending).rejects.toThrow(/Timed out/);
  });

  it("delivers page events to subscribers", () => {
    const { bridge, fake } = setup();
    const seen = vi.fn();

    bridge.onEvent(seen);

    fake.deliver({
      source: BRIDGE_SOURCE,
      direction: "event",
      event: {
        type: "ALERT",
        message: "Training logged successfully",
        at: "2026-08-09T00:00:00.000Z",
      },
    });

    expect(seen).toHaveBeenCalledWith(
      expect.objectContaining({ type: "ALERT" }),
    );
  });

  it("stops calling a handler after it unsubscribes", () => {
    const { bridge, fake } = setup();
    const seen = vi.fn();

    bridge.onEvent(seen)();

    fake.deliver({
      source: BRIDGE_SOURCE,
      direction: "event",
      event: { type: "ALERT", message: "ignored", at: "2026-08-09T00:00:00Z" },
    });

    expect(seen).not.toHaveBeenCalled();
  });

  it("registers one listener no matter how often start runs", () => {
    const { bridge, fake } = setup();

    bridge.start();
    bridge.start();

    const seen = vi.fn();
    bridge.onEvent(seen);

    fake.deliver({
      source: BRIDGE_SOURCE,
      direction: "event",
      event: { type: "ALERT", message: "once", at: "2026-08-09T00:00:00Z" },
    });

    expect(seen).toHaveBeenCalledTimes(1);
  });
});

describe("BridgeClient dialog arming", () => {
  /** Answer every request the client posts, so arm/disarm can settle. */
  function autoAnswer(fake: FakeWindow) {
    const original = fake.postMessage.bind(fake);

    fake.postMessage = (message: unknown, origin: string) => {
      original(message, origin);

      if (isBridgeRequest(message)) {
        queueMicrotask(() => {
          fake.deliver(response(message.id, { success: true }));
        });
      }
    };
  }

  function commandsFrom(fake: FakeWindow) {
    return fake.posted
      .filter(isBridgeRequest)
      .map((request) => (request.command as { type: string }).type);
  }

  it("arms before the operation and disarms after it", async () => {
    const { bridge, fake } = setup();
    autoAnswer(fake);

    const order: string[] = [];

    await bridge.withDialogsArmed(1000, () => {
      order.push(...commandsFrom(fake));
      return Promise.resolve("done");
    });

    // Armed before the body ran, and disarmed only once it had finished.
    expect(order).toEqual(["BRIDGE_ARM_DIALOGS"]);
    expect(commandsFrom(fake)).toEqual([
      "BRIDGE_ARM_DIALOGS",
      "BRIDGE_DISARM_DIALOGS",
    ]);
  });

  it("disarms even when the operation throws", async () => {
    const { bridge, fake } = setup();
    autoAnswer(fake);

    await expect(
      bridge.withDialogsArmed(1000, () =>
        Promise.reject(new Error("submit failed")),
      ),
    ).rejects.toThrow("submit failed");

    // A failed submit must not leave the portal's own dialogs suppressed.
    expect(commandsFrom(fake)).toContain("BRIDGE_DISARM_DIALOGS");
  });

  it("returns the operation result unchanged", async () => {
    const { bridge, fake } = setup();
    autoAnswer(fake);

    await expect(
      bridge.withDialogsArmed(1000, () => Promise.resolve({ ok: 1 })),
    ).resolves.toEqual({ ok: 1 });
  });

  it("still resolves when disarming times out", async () => {
    const fake = new FakeWindow();
    const bridge = new BridgeClient(fake as unknown as Window, 20);
    bridge.start();

    // Answer the arm request only. The disarm goes unanswered, standing in for
    // a MAIN half that died mid-operation.
    const original = fake.postMessage.bind(fake);

    fake.postMessage = (message: unknown, origin: string) => {
      original(message, origin);

      if (
        isBridgeRequest(message) &&
        (message.command as { type: string }).type === "BRIDGE_ARM_DIALOGS"
      ) {
        queueMicrotask(() => {
          fake.deliver(response(message.id, { success: true }));
        });
      }
    };

    // The arm deadline in the MAIN world is what restores the page here, so a
    // lost disarm must not surface as a failed submission.
    await expect(
      bridge.withDialogsArmed(1000, () => Promise.resolve("submitted")),
    ).resolves.toBe("submitted");
  });
});

describe("isBridgeMessage", () => {
  it("accepts well-formed frames", () => {
    expect(
      isBridgeMessage({ source: BRIDGE_SOURCE, direction: "request", id: "1" }),
    ).toBe(true);

    expect(
      isBridgeMessage({
        source: BRIDGE_SOURCE,
        direction: "event",
        event: { type: "ALERT" },
      }),
    ).toBe(true);
  });

  it("rejects anything else the page might post", () => {
    expect(isBridgeMessage(null)).toBe(false);
    expect(isBridgeMessage("string")).toBe(false);
    expect(isBridgeMessage({ source: "someone-else" })).toBe(false);
    expect(isBridgeMessage({ source: BRIDGE_SOURCE })).toBe(false);
  });
});
