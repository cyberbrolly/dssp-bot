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
): string | null {
  if (!checkpoint) return null;
  const unconfirmed = checkpoint.results.filter(
    (row) => row.outcome === "indeterminate",
  ).length;
  if (
    checkpoint.status === "finished" &&
    !unconfirmed &&
    !checkpoint.inFlight &&
    !checkpoint.pending.length
  )
    return null;
  const parts = [
    `Daemon checkpoint: ${checkpoint.status}; started ${displayTimestamp(checkpoint.startedAt)}.`,
    `${checkpoint.results.length} of ${checkpoint.total} results recorded; ${checkpoint.pending.length} pending in the daemon (not proof they were never submitted).`,
  ];
  if (checkpoint.inFlight)
    parts.push(
      `In flight: ${checkpoint.inFlight} — may have reached the portal; verify before another start.`,
    );
  if (unconfirmed)
    parts.push(
      `${unconfirmed} submitted without confirmation — verify on the portal.`,
    );
  parts.push(
    "Only Rust decides recovery/admission. Resume is not available in Stage 29.",
  );
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

export function sessionLocked(state: string): boolean {
  return !["idle", "stopped", "complete"].includes(state);
}
