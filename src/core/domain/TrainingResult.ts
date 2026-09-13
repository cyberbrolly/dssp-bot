import type { ErrorCode } from "../shared/errors";

/**
 * `indeterminate` means the form was submitted but the outcome could not be
 * read back. The record may or may not exist on the portal, so it needs a human
 * to check rather than an automatic retry.
 */
export type TrainingOutcome =
  | "success"
  | "failed"
  | "skipped"
  | "indeterminate";

export interface TrainingResult {
  traineeId: string;
  traineeName: string;
  outcome: TrainingOutcome;
  attempts: number;
  startedAt: string;
  finishedAt: string;
  errorCode?: ErrorCode;
  errorMessage?: string;
}
