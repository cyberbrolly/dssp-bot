import { describe, expect, it } from "vitest";

import {
  DialogGate,
  MAX_ARM_MS,
} from "../../src/core/infrastructure/portal/DialogGate";

/** Controllable clock, so expiry is asserted rather than waited out. */
function clock(start = 1_000_000) {
  let now = start;

  return {
    now: () => now,
    advance: (ms: number) => {
      now += ms;
    },
  };
}

describe("DialogGate", () => {
  it("starts disarmed", () => {
    // The default matters more than it looks: a gate that defaulted to armed
    // would auto-accept the portal's own delete confirmations from page load.
    expect(new DialogGate().armed).toBe(false);
  });

  it("arms for the requested ttl", () => {
    const time = clock();
    const gate = new DialogGate(time.now);

    expect(gate.arm(1000)).toBe(1000);
    expect(gate.armed).toBe(true);
  });

  it("expires on its own once the ttl elapses", () => {
    const time = clock();
    const gate = new DialogGate(time.now);

    gate.arm(1000);
    time.advance(999);
    expect(gate.armed).toBe(true);

    time.advance(1);

    // Nothing sent a disarm here. Recovery has to be automatic, because the
    // half that would have sent it may no longer exist.
    expect(gate.armed).toBe(false);
  });

  it("clamps an over-long ttl", () => {
    const time = clock();
    const gate = new DialogGate(time.now);

    expect(gate.arm(MAX_ARM_MS * 10)).toBe(MAX_ARM_MS);

    time.advance(MAX_ARM_MS);
    expect(gate.armed).toBe(false);
  });

  it.each([
    ["zero", 0],
    ["negative", -1000],
    ["NaN", Number.NaN],
    ["Infinity", Number.POSITIVE_INFINITY],
    ["a string", "1000"],
    ["undefined", undefined],
    ["null", null],
    ["an object", { ttlMs: 1000 }],
  ])("refuses to arm on %s", (_label, value) => {
    const gate = new DialogGate(clock().now);

    expect(gate.arm(value)).toBe(0);
    expect(gate.armed).toBe(false);
  });

  it("disarms", () => {
    const time = clock();
    const gate = new DialogGate(time.now);

    gate.arm(10_000);
    gate.disarm();

    expect(gate.armed).toBe(false);
  });

  it("treats disarm as idempotent", () => {
    const gate = new DialogGate(clock().now);

    gate.disarm();
    gate.disarm();

    expect(gate.armed).toBe(false);
  });

  it("does not let an earlier arm cut a later one short", () => {
    const time = clock();
    const gate = new DialogGate(time.now);

    gate.arm(5000);
    time.advance(1000);
    gate.arm(5000);

    // Second arm extends to t+6000. Had the later call simply overwritten the
    // deadline with the shorter remaining window, dialogs would come back while
    // the second submission was still in flight.
    time.advance(4001);
    expect(gate.armed).toBe(true);

    time.advance(999);
    expect(gate.armed).toBe(false);
  });

  it("ignores an arm that would shorten the current window", () => {
    const time = clock();
    const gate = new DialogGate(time.now);

    gate.arm(10_000);
    gate.arm(100);

    time.advance(101);
    expect(gate.armed).toBe(true);
  });

  it("can be re-armed after expiry", () => {
    const time = clock();
    const gate = new DialogGate(time.now);

    gate.arm(1000);
    time.advance(2000);
    expect(gate.armed).toBe(false);

    gate.arm(1000);
    expect(gate.armed).toBe(true);
  });
});
