import { describe, expect, it } from "vitest";

import {
  isLive,
  markInterrupted,
  unreconciled,
  type BatchCheckpoint,
} from "../../src/core/automation/BatchCheckpoint";
import type { TrainingResult } from "../../src/core/domain/TrainingResult";

function result(
  traineeId: string,
  outcome: TrainingResult["outcome"],
): TrainingResult {
  return {
    traineeId,
    traineeName: `Trainee ${traineeId}`,
    outcome,
    attempts: 1,
    startedAt: "2026-08-11T09:00:00.000Z",
    finishedAt: "2026-08-11T09:00:05.000Z",
  };
}

function checkpoint(overrides: Partial<BatchCheckpoint> = {}): BatchCheckpoint {
  return {
    status: "running",
    startedAt: "2026-08-11T09:00:00.000Z",
    updatedAt: "2026-08-11T09:05:00.000Z",
    total: 3,
    results: [],
    pending: [],
    ...overrides,
  };
}

describe("markInterrupted", () => {
  it.each(["running", "paused"] as const)(
    "re-marks a %s checkpoint as interrupted",
    (status) => {
      // Only a killed worker can leave a live status behind: a worker that ran
      // to completion would have written a terminal one.
      expect(markInterrupted(checkpoint({ status })).status).toBe(
        "interrupted",
      );
    },
  );

  it.each(["finished", "interrupted"] as const)(
    "leaves a %s checkpoint alone",
    (status) => {
      const stored = checkpoint({ status });

      // Same reference, so the caller can skip a pointless storage write.
      expect(markInterrupted(stored)).toBe(stored);
    },
  );

  it("preserves the results of the interrupted batch", () => {
    const stored = checkpoint({
      status: "paused",
      results: [result("a", "success"), result("b", "indeterminate")],
      pending: ["c"],
    });

    const recovered = markInterrupted(stored);

    // These are the records already written to the portal. Losing them is the
    // actual cost of a terminated worker.
    expect(recovered.results).toEqual(stored.results);
    expect(recovered.pending).toEqual(["c"]);
    expect(recovered.total).toBe(3);
  });

  it("does not mutate the stored checkpoint", () => {
    const stored = checkpoint({ status: "running" });

    markInterrupted(stored);

    expect(stored.status).toBe("running");
  });
});

describe("isLive", () => {
  it.each([
    ["running", true],
    ["paused", true],
    ["finished", false],
    ["interrupted", false],
  ] as const)("reports %s as live=%s", (status, expected) => {
    expect(isLive(checkpoint({ status }))).toBe(expected);
  });
});

describe("unreconciled", () => {
  it("separates unconfirmed submissions from untried trainees", () => {
    const stored = checkpoint({
      results: [
        result("a", "success"),
        result("b", "indeterminate"),
        result("c", "failed"),
        result("d", "skipped"),
      ],
      pending: ["e", "f"],
    });

    const { indeterminate, unprocessed } = unreconciled(stored);

    // Only 'b' may have written a record nobody can see. 'c' and 'd' are known
    // not to have, so they are not an operator's problem.
    expect(indeterminate.map((entry) => entry.traineeId)).toEqual(["b"]);
    expect(unprocessed).toEqual(["e", "f"]);
  });

  it("returns empty groups for a clean batch", () => {
    const stored = checkpoint({
      results: [result("a", "success"), result("b", "success")],
      pending: [],
    });

    expect(unreconciled(stored)).toEqual({
      indeterminate: [],
      unprocessed: [],
    });
  });

  it("copies pending rather than aliasing it", () => {
    const stored = checkpoint({ pending: ["x"] });

    unreconciled(stored).unprocessed.push("y");

    expect(stored.pending).toEqual(["x"]);
  });
});
