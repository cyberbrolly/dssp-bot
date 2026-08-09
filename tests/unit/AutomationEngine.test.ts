import { describe, expect, it } from "vitest";
import {
  AutomationEngine,
  type BatchTask,
} from "../../src/core/automation/AutomationEngine";
import { RetryPolicy } from "../../src/core/shared/retry";
import {
  NetworkError,
  SessionExpiredError,
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

describe("AutomationEngine", () => {
  it("processes every trainee sequentially and reports success", async () => {
    const { engine, portal } = engineWith();

    engine.addTasks(tasks("1", "2", "3"));

    const result = await engine.run();

    expect(result.success).toBe(true);
    expect(portal.opened).toEqual(["1", "2", "3"]);
    expect(portal.submitted).toEqual(["1", "2", "3"]);

    if (result.success) {
      expect(result.data.total).toBe(3);
      expect(result.data.successful).toBe(3);
      expect(result.data.successRate).toBe(1);
    }

    expect(engine.getState()).toBe("complete");
  });

  it("refuses to start a second concurrent run", async () => {
    const { engine } = engineWith();

    engine.addTasks(tasks("1", "2"));

    const first = engine.run();
    const second = await engine.run();

    expect(second.success).toBe(false);

    if (!second.success) {
      expect(second.error.message).toMatch(/already running/i);
    }

    await first;
  });

  it("halts the queue while paused and continues after resume", async () => {
    let engineRef: AutomationEngine | null = null;

    const { engine, portal } = engineWith({
      onOpenTrainee: (current) => {
        if (current.id === "1") {
          engineRef?.pause();
        }
      },
    });

    engineRef = engine;
    engine.addTasks(tasks("1", "2", "3"));

    const run = engine.run();

    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(portal.opened).toEqual(["1"]);
    expect(engine.getState()).toBe("paused");

    engine.resume();

    const result = await run;

    expect(result.success).toBe(true);
    expect(portal.opened).toEqual(["1", "2", "3"]);
  });

  it("stops the batch and records the remainder as skipped", async () => {
    let engineRef: AutomationEngine | null = null;

    const { engine, portal } = engineWith({
      onOpenTrainee: (current) => {
        if (current.id === "1") {
          engineRef?.stop();
        }
      },
    });

    engineRef = engine;
    engine.addTasks(tasks("1", "2", "3"));

    const result = await engine.run();

    expect(portal.opened).toEqual(["1"]);
    expect(engine.getState()).toBe("stopped");

    if (result.success) {
      expect(result.data.skipped).toBe(2);
      expect(result.data.total).toBe(3);
    }
  });

  it("stops a paused batch without processing the remainder", async () => {
    let engineRef: AutomationEngine | null = null;

    const { engine, portal } = engineWith({
      onOpenTrainee: (current) => {
        if (current.id === "1") {
          engineRef?.pause();
        }
      },
    });

    engineRef = engine;
    engine.addTasks(tasks("1", "2"));

    const run = engine.run();

    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(engine.getState()).toBe("paused");

    engine.stop();

    const result = await run;

    expect(portal.opened).toEqual(["1"]);
    expect(engine.getState()).toBe("stopped");
    expect(result.success).toBe(true);
  });

  it("retries a recoverable failure and then succeeds", async () => {
    const { engine, portal } = engineWith({
      failOpenTrainee: (id, attempt) =>
        id === "1" && attempt === 1 ? new NetworkError("temporary") : null,
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    expect(portal.opened).toEqual(["1", "1"]);

    if (result.success) {
      expect(result.data.successful).toBe(1);
      expect(result.data.results[0]?.attempts).toBe(2);
    }
  });

  it("does not retry a non-recoverable failure", async () => {
    const { engine, portal } = engineWith({
      failOpenTrainee: (id) => (id === "1" ? new SessionExpiredError() : null),
    });

    engine.addTasks(tasks("1", "2"));

    const result = await engine.run();

    expect(portal.opened).toEqual(["1"]);

    if (result.success) {
      expect(result.data.failed).toBe(1);
      expect(result.data.results[0]?.errorCode).toBe("SESSION_EXPIRED");
    }
  });

  it("aborts the remaining queue once the session expires", async () => {
    const { engine, portal } = engineWith({
      failOpenTrainee: (id) => (id === "1" ? new SessionExpiredError() : null),
    });

    engine.addTasks(tasks("1", "2", "3"));

    const result = await engine.run();

    expect(portal.opened).toEqual(["1"]);

    if (result.success) {
      expect(result.data.failed).toBe(1);
      expect(result.data.skipped).toBe(2);
    }
  });

  it("records a rejected submission as failed", async () => {
    const { engine } = engineWith({
      outcomeFor: () => ({
        status: "rejected",
        message: "Required field missing",
      }),
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    if (result.success) {
      expect(result.data.failed).toBe(1);
      expect(result.data.results[0]?.errorCode).toBe("SUBMISSION_FAILED");
    }
  });

  it("does not resubmit after a duplicate record is detected", async () => {
    const { engine, portal } = engineWith({
      outcomeFor: () => ({
        status: "duplicate",
        message: "Already logged",
      }),
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    expect(portal.submitted).toEqual(["1"]);

    if (result.success) {
      expect(result.data.results[0]?.errorCode).toBe("DUPLICATE_RECORD");
    }
  });

  it("fails fast when the tab is not a portal page", async () => {
    const { engine, portal } = engineWith({
      isPortalPage: false,
    });

    engine.addTasks(tasks("1"));

    const result = await engine.run();

    expect(portal.opened).toEqual([]);

    if (result.success) {
      expect(result.data.results[0]?.errorCode).toBe("SESSION_EXPIRED");
    }
  });

  it("exposes progress counters during a batch", async () => {
    const { engine } = engineWith();

    engine.addTasks(tasks("1", "2"));

    expect(engine.getProgress().total).toBe(2);
    expect(engine.getProgress().remaining).toBe(2);

    await engine.run();

    const progress = engine.getProgress();

    expect(progress.processed).toBe(2);
    expect(progress.successful).toBe(2);
    expect(progress.remaining).toBe(0);
  });
});
