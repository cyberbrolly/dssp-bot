import type { TrainingResult } from "../domain/TrainingResult";

/**
 * Lifecycle of a persisted batch.
 *
 * `running` and `paused` are live states, so finding either one in storage on
 * worker startup means the previous worker did not get to write a terminal
 * state — it was killed. `interrupted` records that conclusion.
 */
export type BatchCheckpointStatus =
  "running" | "paused" | "finished" | "interrupted";

/**
 * A durable snapshot of batch progress.
 *
 * The engine keeps its queue and results in service worker memory, which
 * Chromium reclaims after roughly 30 seconds of inactivity. Everything needed
 * to answer "what did this batch already write to the portal?" is mirrored here
 * after every trainee, because that question outlives the worker.
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
 * Receives each checkpoint the engine produces.
 *
 * Injected rather than imported so the engine stays free of storage concerns
 * and remains testable without a browser. Implementations own their own error
 * reporting — the engine ignores rejections so a storage fault cannot abort a
 * batch that is otherwise submitting successfully.
 */
export type CheckpointWriter = (
  checkpoint: BatchCheckpoint,
) => void | Promise<void>;

/** True while the batch that wrote this checkpoint was still expected to run. */
export function isLive(checkpoint: BatchCheckpoint): boolean {
  return LIVE_STATUSES.has(checkpoint.status);
}

/**
 * Re-mark a checkpoint abandoned by a terminated worker.
 *
 * Returns the input unchanged when the status is already terminal, so callers
 * can use referential equality to tell a genuine recovery from a no-op and skip
 * a redundant write.
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
