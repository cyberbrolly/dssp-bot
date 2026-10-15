import { describe, expect, it, vi } from "vitest";
import { DaemonClient } from "../../src/core/infrastructure/daemon/DaemonClient";
import type { BatchProgress } from "../../src/core/infrastructure/messaging/Messages";
import type {
  DaemonCheckpoint,
  DaemonReport,
} from "../../src/core/infrastructure/daemon/DaemonClient";
import {
  displayTimestamp,
  readDaemonView,
  pauseSummary,
  stopSummary,
  recoverySummary,
  reportSummary,
  sessionLocked,
} from "../../src/popup/daemonView";

const checkpoint: DaemonCheckpoint = {
  status: "aborted",
  startedAt: "1789382400000",
  updatedAt: "1789382401000",
  total: 2,
  pending: ["fixture-pending"],
  inFlight: "fixture-in-flight",
  results: [],
};
const report: DaemonReport = {
  total: 1,
  successful: 0,
  failed: 1,
  skipped: 0,
  indeterminate: 0,
  successRate: 0,
  aborted: true,
  startedAt: "1789382400000",
  finishedAt: "1789382401000",
  results: [
    {
      traineeId: "fixture",
      traineeName: "Fixture",
      outcome: "failed",
      attempts: 1,
    },
  ],
};
const idle: BatchProgress = {
  state: "idle",
  total: 0,
  processed: 0,
  successful: 0,
  failed: 0,
  skipped: 0,
  indeterminate: 0,
  remaining: 0,
};

describe("daemon UI projections", () => {
  it("polls Rust afresh even when idle after restart; preserves durable inFlight/aborted/epoch strings", async () => {
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url) => {
      const value = String(url).endsWith("/status")
        ? idle
        : String(url).endsWith("/report")
          ? null
          : checkpoint;
      return new Response(JSON.stringify(value));
    });
    const client = new DaemonClient(
      { port: 8765, token: "a".repeat(64) },
      fetcher,
    );
    const view = await readDaemonView(client);
    expect(view).toEqual({ progress: idle, report: null, checkpoint });
    const before = JSON.stringify(view.checkpoint);
    expect(recoverySummary(view.checkpoint)).toContain(
      "may have reached the portal",
    );
    expect(recoverySummary(view.checkpoint)).toContain(
      "not proof they were never submitted",
    );
    expect(JSON.stringify(view.checkpoint)).toBe(before);
    await readDaemonView(client);
    expect(fetcher).toHaveBeenCalledTimes(6);
    expect(
      fetcher.mock.calls.every(
        ([url]) => !String(url).includes("form-options"),
      ),
    ).toBe(true);
  });
  it("does not replace a corrupt checkpoint or disconnected daemon with empty local state", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockImplementation(
        async (url) =>
          new Response(
            JSON.stringify(
              String(url).endsWith("/checkpoint")
                ? { error: "corrupt checkpoint" }
                : String(url).endsWith("/status")
                  ? idle
                  : null,
            ),
            { status: String(url).endsWith("/checkpoint") ? 500 : 200 },
          ),
      );
    await expect(
      readDaemonView(
        new DaemonClient({ port: 8765, token: "a".repeat(64) }, fetcher),
      ),
    ).rejects.toThrow("corrupt checkpoint");
    expect(fetcher).toHaveBeenCalledTimes(3);
  });
  it("handles epoch and legacy ISO display safely without altering wire data", () => {
    expect(displayTimestamp("1789382400000")).toBe("2026-09-14 10:40:00.000Z");
    expect(displayTimestamp("2026-09-14T00:00:00Z")).toBe(
      "2026-09-14 00:00:00.000Z",
    );
    expect(displayTimestamp("invalid")).toBe("an unknown time");
    expect(displayTimestamp("999999999999999999999999999")).toBe(
      "an unknown time",
    );
  });
  it("shows aborted reports even with zero skipped, and never calls a 202 a finished report", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValue(new Response(JSON.stringify(report)));
    expect(
      reportSummary(
        (await new DaemonClient(
          { port: 8765, token: "a".repeat(64) },
          fetcher,
        ).report())!,
      ),
    ).toContain("Batch aborted; human review");
    expect(reportSummary({ ...report, indeterminate: 1 })).toContain(
      "submitted without confirmation",
    );
  });
  it("does not infer interruption or modify an active checkpoint", () => {
    const active = { ...checkpoint, status: "running" as const };
    expect(recoverySummary(active)).toContain("checkpoint: running");
    expect(active.status).toBe("running");
    expect(recoverySummary(null)).toBeNull();
    expect(
      recoverySummary({
        ...checkpoint,
        status: "finished",
        pending: [],
        inFlight: null,
      }),
    ).toBeNull();
  });
  it("locks active, pending, paused, unknown and disconnected phases", () => {
    for (const state of [
      "initializing",
      "submitting",
      "loading_trainee",
      "retrying",
      "paused",
      "unknown",
      "disconnected",
    ])
      expect(sessionLocked(state)).toBe(true);
    for (const state of ["idle", "complete", "stopped"])
      expect(sessionLocked(state)).toBe(false);
  });

  it("distinguishes active paused checkpoints from restart recovery, without mutating them", () => {
    const paused = { ...checkpoint, status: "paused" as const, inFlight: null };
    const before = JSON.stringify(paused);
    const live = recoverySummary(paused, {
      ...idle,
      state: "paused",
      pauseRequested: true,
    });
    expect(live).toContain("in-memory paused batch, not an interrupted batch");
    expect(live).toContain("same batch without new admission or a new worker");
    expect(live).not.toContain("Resume cannot recover");
    const restarted = recoverySummary(paused, idle);
    expect(restarted).toContain(
      "durable checkpoint alone is not a resumable in-memory batch",
    );
    expect(restarted).toContain(
      "resume: true and the original session template",
    );
    expect(restarted).not.toContain("Resume is not available");
    expect(
      recoverySummary({ ...paused, status: "interrupted" }, idle),
    ).toContain("checkpoint: interrupted");
    expect(
      recoverySummary(paused, {
        ...idle,
        state: "submitting",
        pauseRequested: true,
      }),
    ).toContain("active batch, not an interrupted batch");
    expect(JSON.stringify(paused)).toBe(before);
  });

  it("describes pending intent, durable pause and resume wake-up without optimistic local state", () => {
    expect(
      pauseSummary({ ...idle, state: "submitting", pauseRequested: true }),
    ).toContain("Pause pending");
    expect(
      pauseSummary({ ...idle, state: "submitting", pauseRequested: true }),
    ).toContain("does not cancel an in-flight POST");
    expect(
      pauseSummary({ ...idle, state: "paused", pauseRequested: true }),
    ).toContain("checkpoint was persisted");
    expect(
      pauseSummary({ ...idle, state: "paused", pauseRequested: false }),
    ).toContain("waiting for Rust to persist the running checkpoint");
    expect(pauseSummary({ ...idle, state: "paused" })).toContain(
      "same in-memory batch",
    );
    expect(
      pauseSummary({ ...idle, state: "submitting", pauseRequested: false }),
    ).toBeNull();
    expect(pauseSummary(idle)).toBeNull();
  });

  it.each(["loading_trainee", "submitting", "retrying", "paused"] as const)(
    "describes latched stop in %s as pending, never as a durable terminal or a resumable pause",
    (state) => {
      const progress = {
        ...idle,
        state,
        stopRequested: true,
        pauseRequested: true,
      };
      const before = JSON.stringify(progress);
      expect(stopSummary(progress)).toContain("Stop pending");
      expect(stopSummary(progress)).toContain(
        "does not cancel an in-flight POST",
      );
      expect(stopSummary(progress)).toContain("No new attempt or safe retry");
      expect(stopSummary(progress)).not.toContain("Rust reports stopped");
      expect(pauseSummary(progress)).toBeNull();
      const text = recoverySummary(
        { ...checkpoint, status: "paused" },
        progress,
      );
      expect(text).toContain("still pending");
      expect(text).toContain("Pause and Resume cannot cancel");
      expect(text).not.toContain("Resume continues");
      expect(text).not.toContain("resume: true");
      expect(JSON.stringify(progress)).toBe(before);
    },
  );

  it.each(["running", "paused", "aborted", "finished"] as const)(
    "keeps human-review and UI lock guidance for a terminal latch despite a %s checkpoint",
    (status) => {
      const durable = { ...checkpoint, status, pending: [], inFlight: null };
      const text = recoverySummary(durable, {
        ...idle,
        state: "stopped",
        stopRequested: true,
      });
      expect(text).toContain(
        "stopped with stop latched, not successful completion",
      );
      expect(text).toContain("Start and session edits stay locked");
      expect(text).toContain("explicit external admission after human review");
      expect(text).toContain("Rust's recovery guards");
      expect(text).not.toContain("resume: true");
      expect(text).not.toContain("still pending");
      expect(
        stopSummary({ ...idle, state: "stopped", stopRequested: true }),
      ).toContain("Rust reports stopped");
    },
  );

  it("warns on null reports after checkpoint failure without fabricating results or assuming a missing checkpoint is safe", () => {
    const progress = {
      ...idle,
      state: "stopped" as const,
      stopRequested: true,
    };
    for (const durable of [checkpoint, null]) {
      const before = JSON.stringify(durable);
      const text = recoverySummary(durable, progress, null);
      expect(text).toContain("No final report is available");
      expect(text).toContain("checkpoint write or execution failure");
      expect(text).toContain(
        "counts may reflect only the last durable checkpoint",
      );
      expect(text).toContain("Human review is required");
      expect(text).toContain("no automatic recovery");
      expect(text).not.toContain("resume: true");
      expect(JSON.stringify(durable)).toBe(before);
    }
    expect(
      recoverySummary(null, { ...idle, state: "stopped" }, null),
    ).toContain("No final report");
    expect(recoverySummary(null, idle, null)).toBeNull();
    expect(
      recoverySummary(null, { ...idle, state: "paused", stopRequested: true }),
    ).toContain("still pending");
  });

  it("preserves last-row success in an aborted report and never relabels normal completion", () => {
    const completed: DaemonReport = {
      ...report,
      successful: 1,
      failed: 0,
      successRate: 1,
      aborted: false,
      results: [{ ...report.results[0]!, outcome: "success" }],
    };
    const before = JSON.stringify(completed);
    expect(reportSummary(completed)).toBe(
      "1/1 recorded, 0 failed, 0 skipped, 100% success rate.",
    );
    const stopped = { ...completed, aborted: true };
    expect(reportSummary(stopped)).toContain(
      "1/1 recorded, 0 failed, 0 skipped, 100% success rate.",
    );
    expect(reportSummary(stopped)).toContain(
      "Batch aborted; human review is required, even if no trainees were skipped",
    );
    expect(stopped.results[0]?.outcome).toBe("success");
    expect(JSON.stringify(completed)).toBe(before);
    expect(
      stopSummary({ ...idle, state: "complete", stopRequested: false }),
    ).toBeNull();
  });
});
