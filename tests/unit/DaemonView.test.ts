import { describe, expect, it, vi } from "vitest";
import { DaemonClient } from "../../src/core/infrastructure/daemon/DaemonClient";
import type {
  DaemonCheckpoint,
  DaemonReport,
} from "../../src/core/infrastructure/daemon/DaemonClient";
import {
  displayTimestamp,
  readDaemonView,
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
const idle = {
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
  it("locks active/unknown/disconnected phases; controls never implement pause/resume/stop", () => {
    for (const state of [
      "initializing",
      "submitting",
      "loading_trainee",
      "paused",
      "unknown",
      "disconnected",
    ])
      expect(sessionLocked(state)).toBe(true);
    for (const state of ["idle", "complete", "stopped"])
      expect(sessionLocked(state)).toBe(false);
  });
});
