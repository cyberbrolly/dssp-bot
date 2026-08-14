import { describe, expect, it, vi } from "vitest";

import { BatchRunner } from "../../src/core/automation/BatchRunner";
import {
  AutomationEngine,
  type AutomationEngine as AutomationEngineType,
} from "../../src/core/automation/AutomationEngine";
import { FakePortalAdapter, trainee } from "./FakePortalAdapter";
import { NetworkError } from "../../src/core/shared/errors";
import { RetryPolicy } from "../../src/core/shared/retry";

const session = {
  instructorId: "instructor",
  trainingTypeId: "training",
  trainingDate: "2026-08-13",
};

function harness(name: string) {
  const record = trainee("5578387");
  record.name = name;
  const portal = new FakePortalAdapter({ trainees: [record] });
  const addTasks = vi.fn();
  const engine = {
    clearQueue: vi.fn(),
    addTasks,
    run: vi.fn().mockResolvedValue({ success: true, data: {} }),
  } as unknown as AutomationEngineType;

  return { runner: new BatchRunner(portal, engine), addTasks, record };
}

describe("BatchRunner trainee resolution", () => {
  it("resolves a known trainee ID", async () => {
    const { runner, addTasks, record } = harness("ogbonna Victor chinoso");

    await runner.start({ traineeIds: ["5578387"], session });

    expect(addTasks).toHaveBeenCalledWith([
      expect.objectContaining({ trainee: record }),
    ]);
  });

  it.each([
    ["embedded newlines", "ogbonna Victor\nchinoso"],
    ["non-breaking space", "ogbonna Victor\u00a0chinoso"],
    ["multiple spaces", "ogbonna Victor   chinoso"],
  ])("resolves names with %s", async (_description, scrapedName) => {
    const { runner, addTasks, record } = harness("ogbonna Victor chinoso");
    record.name = scrapedName;

    await runner.start({
      traineeIds: ["ogbonna victor chinoso"],
      session,
    });

    expect(addTasks).toHaveBeenCalledWith([
      expect.objectContaining({ trainee: record }),
    ]);
  });

  it("normalizes a day-first date before queuing submissions", async () => {
    const { runner, addTasks } = harness("Trainee 5578387");

    await runner.start({
      traineeIds: ["5578387"],
      session: { ...session, trainingDate: "14/08/2026" },
    });

    const firstCall = (addTasks.mock.calls as unknown[][])[0];
    const queued = firstCall?.[0];

    expect(Array.isArray(queued)).toBe(true);

    if (Array.isArray(queued)) {
      const firstTask = queued[0] as {
        session?: { trainingDate?: string };
      };

      expect(firstTask.session?.trainingDate).toBe("2026-08-14");
    }
  });

  it("reports an invalid numeric ID without aborting valid trainees", async () => {
    const portal = new FakePortalAdapter({
      trainees: [trainee("1"), trainee("2")],
      failOpenTrainee: (id) =>
        id === "999"
          ? new NetworkError("Trainee form returned HTTP 404.")
          : null,
    });
    const engine = new AutomationEngine({
      portal,
      retryPolicy: new RetryPolicy({
        initialDelayMs: 0,
        maxDelayMs: 0,
        maxAttempts: 1,
      }),
      interTaskDelayMs: 0,
    });
    const runner = new BatchRunner(portal, engine);

    const result = await runner.start({
      traineeIds: ["1", "999", "2"],
      session,
    });

    expect(result.success).toBe(true);

    if (result.success) {
      expect(result.data.successful).toBe(2);
      expect(result.data.failed).toBe(1);
      expect(result.data.results.map((entry) => entry.outcome)).toEqual([
        "success",
        "failed",
        "success",
      ]);
    }
  });
});
