import { describe, expect, it } from "vitest";
import { buildBatchReport } from "../../src/core/domain/BatchReport";
import type { TrainingResult } from "../../src/core/domain/TrainingResult";

function result(
  id: string,
  outcome: TrainingResult["outcome"],
): TrainingResult {
  return {
    traineeId: id,
    traineeName: `Trainee ${id}`,
    outcome,
    attempts: 1,
    startedAt: "2026-01-01T00:00:00.000Z",
    finishedAt: "2026-01-01T00:00:05.000Z",
  };
}

describe("buildBatchReport", () => {
  it("counts each outcome", () => {
    const report = buildBatchReport(
      [
        result("1", "success"),
        result("2", "success"),
        result("3", "failed"),
        result("4", "skipped"),
      ],
      "2026-01-01T00:00:00.000Z",
      "2026-01-01T00:01:00.000Z",
    );

    expect(report.total).toBe(4);
    expect(report.successful).toBe(2);
    expect(report.failed).toBe(1);
    expect(report.skipped).toBe(1);
    expect(report.successRate).toBe(0.5);
  });

  it("reports a zero rate for an empty batch without dividing by zero", () => {
    const report = buildBatchReport(
      [],
      "2026-01-01T00:00:00.000Z",
      "2026-01-01T00:00:00.000Z",
    );

    expect(report.total).toBe(0);
    expect(report.successRate).toBe(0);
    expect(Number.isNaN(report.successRate)).toBe(false);
  });

  it("preserves the underlying results", () => {
    const results = [result("1", "success")];

    const report = buildBatchReport(
      results,
      "2026-01-01T00:00:00.000Z",
      "2026-01-01T00:01:00.000Z",
    );

    expect(report.results).toHaveLength(1);
    expect(report.results[0]?.traineeId).toBe("1");
  });
});
