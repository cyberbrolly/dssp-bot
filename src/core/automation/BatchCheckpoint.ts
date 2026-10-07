import type { TrainingResult } from "../domain/TrainingResult";

/**
 * `running` and `paused` are live; finding either in storage at startup means
 * the previous worker was killed before writing a terminal state, which
 * `interrupted` records.
 */
export type BatchCheckpointStatus =
  "running" | "paused" | "finished" | "interrupted";

/**
 * A durable snapshot of batch progress: the engine's queue and results live in
 * service worker memory, reclaimed after ~30s of inactivity, so everything
 * needed to answer "what did this batch already write?" is mirrored here.
 */
export interface BatchCheckpoint {
  status: BatchCheckpointStatus;
  /** ISO 8601. Matches the report's `startedAt` for the same batch. */
  startedAt: string;
  /** ISO 8601, refreshed on every write. */
  updatedAt: string;
  /** Trainees in the batch as originally queued. */
  total: number;
  /** Results recorded so far, in completion order. */
  results: TrainingResult[];
  /** Trainee ids still queued, in order. Empty once the queue drains. */
  pending: string[];
}

const LIVE_STATUSES: ReadonlySet<BatchCheckpointStatus> = new Set([
  "running",
  "paused",
]);

/**
 * Injected rather than imported so the engine stays free of storage concerns
 * and testable without a browser. Implementations own their error reporting:
 * the engine ignores rejections so a storage fault cannot abort a batch.
 */
export type CheckpointWriter = (
  checkpoint: BatchCheckpoint,
) => void | Promise<void>;

/** True while the batch that wrote this checkpoint was still expected to run. */
export function isLive(checkpoint: BatchCheckpoint): boolean {
  return LIVE_STATUSES.has(checkpoint.status);
}

/**
 * Re-marks a checkpoint abandoned by a terminated worker. Returns the input
 * unchanged when the status is already terminal, so callers can use
 * referential equality to detect a genuine recovery and skip a write.
 */
export function markInterrupted(checkpoint: BatchCheckpoint): BatchCheckpoint {
  if (!isLive(checkpoint)) {
    return checkpoint;
  }

  return { ...checkpoint, status: "interrupted" };
}

/**
 * Split out the trainees a checkpoint cannot account for.
 *
 * These are the two groups that need an operator to look at the portal
 * directly: `indeterminate` records may have been written without a readable
 * confirmation, and `unprocessed` ones never got as far as an attempt. Both are
 * unsafe to blindly resubmit, which is why recovery reports them instead of
 * retrying them.
 */
export function unreconciled(checkpoint: BatchCheckpoint): {
  indeterminate: TrainingResult[];
  unprocessed: string[];
} {
  return {
    indeterminate: checkpoint.results.filter(
      (result) => result.outcome === "indeterminate",
    ),
    unprocessed: [...checkpoint.pending],
  };
}
