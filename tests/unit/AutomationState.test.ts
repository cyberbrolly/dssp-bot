import { describe, expect, it } from "vitest";
import {
  StateMachine,
  canTransition,
  isActive,
} from "../../src/core/automation/AutomationState";

describe("state transitions", () => {
  it("only starts a batch from idle", () => {
    expect(canTransition("idle", "initializing")).toBe(true);
    expect(canTransition("idle", "submitting")).toBe(false);
    expect(canTransition("complete", "submitting")).toBe(false);
  });

  it("follows the documented happy path", () => {
    const machine = new StateMachine();

    const path = [
      "initializing",
      "loading_trainee",
      "opening_form",
      "filling_form",
      "validating",
      "submitting",
      "verifying",
      "complete",
    ] as const;

    for (const next of path) {
      machine.transitionTo(next);
      expect(machine.state).toBe(next);
    }
  });

  it("advances to the next trainee after verifying", () => {
    expect(canTransition("verifying", "loading_trainee")).toBe(true);
  });

  it("rejects illegal transitions", () => {
    const machine = new StateMachine();

    machine.transitionTo("initializing");

    expect(() => machine.transitionTo("idle")).toThrow(
      /Illegal state transition/,
    );
  });

  it("allows pause and stop from every active state", () => {
    const active = [
      "loading_trainee",
      "opening_form",
      "filling_form",
      "validating",
      "submitting",
      "verifying",
      "retrying",
    ] as const;

    for (const state of active) {
      expect(canTransition(state, "paused")).toBe(true);
      expect(canTransition(state, "stopped")).toBe(true);
      expect(isActive(state)).toBe(true);
    }
  });

  it("resumes or stops from paused but cannot resubmit directly", () => {
    expect(canTransition("paused", "loading_trainee")).toBe(true);
    expect(canTransition("paused", "stopped")).toBe(true);
    expect(canTransition("paused", "submitting")).toBe(false);
  });

  it("treats terminal states as inactive", () => {
    expect(isActive("idle")).toBe(false);
    expect(isActive("paused")).toBe(false);
    expect(isActive("stopped")).toBe(false);
    expect(isActive("complete")).toBe(false);
  });

  it("resets back to idle", () => {
    const machine = new StateMachine();

    machine.transitionTo("initializing");
    machine.reset();

    expect(machine.state).toBe("idle");
    expect(machine.isActive).toBe(false);
  });
});
