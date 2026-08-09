import { describe, expect, it } from "vitest";
import {
  AutomationEngine,
  type BatchTask,
} from "../../src/core/automation/AutomationEngine";
import { BatchRunner } from "../../src/core/automation/BatchRunner";
import { RetryPolicy } from "../../src/core/shared/retry";
import {
  NetworkError,
  PortalElementNotFoundError,
  TimeoutError,
} from "../../src/core/shared/errors";
import { FakePortalAdapter, trainee } from "./FakePortalAdapter";
import type { FakePortalOptions } from "./FakePortalAdapter";

const session = {
  trainingDate: "2026-01-15",
  instructorId: "INS-1",
  trainingTypeId: "TT-1",
};

function tasks(...ids: string[]): BatchTask[] {
  return ids.map((id) => ({
    trainee: trainee(id),
    session,
  }));
}

function engineWith(options: FakePortalOptions = {}): {
  engine: AutomationEngine;
  portal: FakePortalAdapter;
} {
  const portal = new FakePortalAdapter(options);

  const engine = new AutomationEngine({
    portal,
    retryPolicy: new RetryPolicy({
      initialDelayMs: 0,
      maxDelayMs: 0,
      maxAttempts: 3,
    }),
    interTaskDelayMs: 0,
  });

  return { engine, portal };
}

/**
 * These records are official training logs. A duplicate is worse than a
 * failure, because a failure is visible and a duplicate is not. Every test here
 * exists to keep the retry policy away from the one call that writes.
 */
describe("submission safety", () => {
  // The original bug: RetryPolicy wrapped the whole cycle, so a confirmation
  // timeout replayed the submit and wrote a second training record.
  it("never submits twice when the confirmation times out", async () => {
    const { engine, portal } = engineWith({
      failConfirm: (_id, call) =>
        call === 1 ? new TimeoutError("confirmation", 100) : null,
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    // Was ["1", "1"] before the submit boundary was split out.
    expect(portal.submitted).toEqual(["1"]);

    if (result.success) {
      // Re-reading the outcome is safe, so the retry recovers cleanly.
      expect(result.data.results[0]?.outcome).toBe("success");
    }
  });

  it("reports an unconfirmable submission as indeterminate, not success", async () => {
    const { engine } = engineWith({
      failConfirm: () => new TimeoutError("confirmation", 100),
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    if (result.success) {
      expect(result.data.successful).toBe(0);
      expect(result.data.failed).toBe(0);
      expect(result.data.indeterminate).toBe(1);
      expect(result.data.results[0]?.errorCode).toBe("CONFIRMATION_UNKNOWN");
    }
  });

  it("retries a recoverable confirmation failure without resubmitting", async () => {
    const { engine, portal } = engineWith({
      failConfirm: (_id, call) =>
        call === 1 ? new NetworkError("dropped") : null,
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    // Confirmation is read-only, so retrying it is safe and should recover.
    expect(portal.submitted).toEqual(["1"]);
    expect(portal.confirmed).toEqual(["1"]);

    if (result.success) {
      expect(result.data.successful).toBe(1);
    }
  });

  it("stops the batch after an unconfirmed submission", async () => {
    const { engine, portal } = engineWith({
      failConfirm: (id) =>
        id === "1" ? new TimeoutError("confirmation", 100) : null,
    });

    engine.addTasks(tasks("1", "2", "3"));

    const result = await engine.run();

    // Continuing would write records nobody can account for.
    expect(portal.submitted).toEqual(["1"]);

    if (result.success) {
      expect(result.data.indeterminate).toBe(1);
      expect(result.data.skipped).toBe(2);
    }
  });

  it("still retries the preparation steps, which write nothing", async () => {
    const { engine, portal } = engineWith({
      failOpenTrainee: (id, attempt) =>
        id === "1" && attempt === 1 ? new NetworkError("temporary") : null,
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    expect(portal.opened).toEqual(["1", "1"]);
    expect(portal.submitted).toEqual(["1"]);

    if (result.success) {
      expect(result.data.successful).toBe(1);
      expect(result.data.results[0]?.attempts).toBe(2);
    }
  });

  it("treats a transport failure during submit as indeterminate", async () => {
    const { engine, portal } = engineWith({
      failSubmit: () => new NetworkError("connection reset"),
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    // The request may have reached the portal, so this cannot be a clean fail.
    expect(portal.submitted).toEqual(["1"]);

    if (result.success) {
      expect(result.data.indeterminate).toBe(1);
      expect(result.data.failed).toBe(0);
    }
  });

  it("treats a missing submit button as a plain failure", async () => {
    const { engine } = engineWith({
      failSubmit: () => new PortalElementNotFoundError("#btn-save-vehicle"),
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    // Nothing was clicked, so nothing could have been recorded.
    if (result.success) {
      expect(result.data.failed).toBe(1);
      expect(result.data.indeterminate).toBe(0);
    }
  });
});

describe("batch counters", () => {
  it("reports the new batch size when a second batch is smaller", async () => {
    const portal = new FakePortalAdapter({
      trainees: [trainee("1"), trainee("2"), trainee("3")],
    });

    const engine = new AutomationEngine({ portal, interTaskDelayMs: 0 });
    const runner = new BatchRunner(portal, engine);

    await runner.start({ traineeIds: ["1", "2", "3"], session });

    expect(engine.getProgress().total).toBe(3);

    await runner.start({ traineeIds: ["1", "2"], session });

    const progress = engine.getProgress();

    expect(progress.total).toBe(2);
    expect(progress.processed).toBe(2);
  });
});
