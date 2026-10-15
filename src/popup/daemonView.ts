import type {
  DaemonCheckpoint,
  DaemonClient,
  DaemonReport,
} from "../core/infrastructure/daemon/DaemonClient";
import type { BatchProgress } from "../core/infrastructure/messaging/Messages";

export interface DaemonView {
  progress: BatchProgress;
  report: DaemonReport | null;
  checkpoint: DaemonCheckpoint | null;
}

// Each refresh reads Rust anew; nothing is cached or persisted as execution state.
export async function readDaemonView(
  client: DaemonClient,
): Promise<DaemonView> {
  const [progress, report, checkpoint] = await Promise.all([
    client.status(),
    client.report(),
    client.checkpoint(),
  ]);
  return { progress, report, checkpoint };
}

export function displayTimestamp(value: string): string {
  const date = new Date(/^\d+$/.test(value) ? Number(value) : value);
  return Number.isFinite(date.getTime())
    ? date.toISOString().replace("T", " ")
    : "an unknown time";
}

export function recoverySummary(
  checkpoint: DaemonCheckpoint | null,
  progress?: BatchProgress,
  report?: DaemonReport | null,
): string | null {
  const stopped = progress?.state === "stopped";
  const stopping = !!progress?.stopRequested;
  const missingReport = stopped && report === null;
  if (!checkpoint && !stopping && !missingReport) return null;
  const parts: string[] = [];
  if (stopping) {
    parts.push(
      stopped
        ? "Rust reports stopped with stop latched, not successful completion. Human review of actual outcomes and the last durable checkpoint is required."
        : "Stop is latched in Rust and still pending. Pause and Resume cannot cancel it or start checkpoint recovery. An admitted attempt must settle before Rust publishes stopped.",
    );
    parts.push(
      "Start and session edits stay locked while Rust reports stopRequested. This popup does not admit checkpoint recovery; any new batch requires explicit external admission after human review, subject to Rust's recovery guards. Nothing is retried automatically.",
    );
  }
  if (missingReport)
    parts.push(
      "No final report is available. A checkpoint write or execution failure may have prevented finalization; progress counts may reflect only the last durable checkpoint. Human review is required; there is no automatic recovery.",
    );
  if (!checkpoint) return parts.join(" ");
  const unconfirmed = checkpoint.results.filter(
    (row) => row.outcome === "indeterminate",
  ).length;
  if (
    checkpoint.status === "finished" &&
    !unconfirmed &&
    !checkpoint.inFlight &&
    !checkpoint.pending.length &&
    !stopping &&
    !missingReport
  )
    return null;
  parts.push(
    `Daemon checkpoint: ${checkpoint.status}; started ${displayTimestamp(checkpoint.startedAt)}.`,
    `${checkpoint.results.length} of ${checkpoint.total} results recorded; ${checkpoint.pending.length} pending in the daemon (not proof they were never submitted).`,
  );
  if (checkpoint.inFlight)
    parts.push(
      `In flight: ${checkpoint.inFlight} — may have reached the portal; verify before another start.`,
    );
  if (unconfirmed)
    parts.push(
      `${unconfirmed} submitted without confirmation — verify on the portal.`,
    );
  if (stopping) return parts.join(" ");
  if (stopped) {
    parts.push(
      "Rust reports stopped, not successful completion. Inspect the report and last durable checkpoint; stopping or an execution error may have aborted the batch. Controls cannot recover it or retry trainees.",
    );
    parts.push(
      "Recovery requires explicit batch admission with resume: true and the original session template after human review; no terminal batch is retried automatically.",
    );
  } else if (progress && sessionLocked(progress.state)) {
    parts.push(
      progress.state === "paused"
        ? "Rust reports an in-memory paused batch, not an interrupted batch. Resume continues that same batch without new admission or a new worker."
        : "Rust reports an active batch, not an interrupted batch. Resume only cancels a pending pause; it does not start checkpoint recovery.",
    );
  } else {
    parts.push(
      "Only Rust decides recovery/admission. A durable checkpoint alone is not a resumable in-memory batch. After a restart, Resume cannot recover it; recovery requires explicit batch admission with resume: true and the original session template after human review.",
    );
  }
  return parts.join(" ");
}

export function reportSummary(report: DaemonReport): string {
  const parts = [
    `${report.successful}/${report.total} recorded, ${report.failed} failed, ${report.skipped} skipped, ${Math.round(report.successRate * 100)}% success rate.`,
  ];
  if (report.aborted)
    parts.push(
      "Batch aborted; human review is required, even if no trainees were skipped.",
    );
  if (report.indeterminate)
    parts.push(
      `${report.indeterminate} submitted without confirmation — check the portal before re-running.`,
    );
  return parts.join(" ");
}

export function stopSummary(progress: BatchProgress): string | null {
  if (progress.state === "stopped")
    return "Rust reports stopped, not successful completion. Review the report and durable checkpoint for aborted work and actual outcomes; nothing is retried automatically.";
  if (progress.stopRequested)
    return "Stop pending — latched in Rust. This does not cancel an in-flight POST or its settlement. No new attempt or safe retry is admitted after stop. Waiting for Rust to publish stopped; Pause and Resume cannot cancel stop.";
  return null;
}

export function pauseSummary(progress: BatchProgress): string | null {
  if (progress.stopRequested) return null;
  if (progress.state === "paused")
    return progress.pauseRequested === false
      ? "Resume requested; waiting for Rust to persist the running checkpoint before further submissions."
      : "Paused at the safe boundary after the checkpoint was persisted. Resume continues the same in-memory batch.";
  if (progress.pauseRequested)
    return "Pause pending — waiting for Rust's safe checkpoint boundary. This does not cancel an in-flight POST or its settlement. Resume cancels the pending pause.";
  return null;
}

export function sessionLocked(state: string): boolean {
  return !["idle", "stopped", "complete"].includes(state);
}
