import type { TrainingResult } from "./TrainingResult";

export interface BatchReport {
  total: number;
  successful: number;
  failed: number;
  skipped: number;
  /** Submitted but unconfirmed. Each one needs manual verification. */
  indeterminate: number;
  successRate: number;
  startedAt: string;
  finishedAt: string;
  results: TrainingResult[];
}

export function buildBatchReport(
  results: TrainingResult[],
  startedAt: string,
  finishedAt: string,
): BatchReport {
  const count = (outcome: TrainingResult["outcome"]): number =>
    results.filter((result) => result.outcome === outcome).length;

  const successful = count("success");
  const total = results.length;

  return {
    total,
    successful,
    failed: count("failed"),
    skipped: count("skipped"),
    indeterminate: count("indeterminate"),
    successRate: total === 0 ? 0 : successful / total,
    startedAt,
    finishedAt,
    results,
  };
}
