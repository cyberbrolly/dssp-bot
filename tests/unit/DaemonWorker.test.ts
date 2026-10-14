import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import type { Message } from "../../src/core/infrastructure/messaging/Messages";
import {
  DAEMON_SETTINGS_KEY,
  proxyDaemonMessage,
} from "../../src/core/infrastructure/daemon/DaemonClient";

const settings = { port: 8765, token: "a".repeat(64) };
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
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("stateless service-worker bridge", () => {
  it("rejects portal/content/foreign callers before credentials are read", async () => {
    vi.resetModules();
    const listener = vi.fn();
    const storage = {
      get: vi.fn().mockResolvedValue({ [DAEMON_SETTINGS_KEY]: settings }),
      set: vi.fn(),
    };
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValue(new Response(JSON.stringify(idle)));
    vi.stubGlobal("fetch", fetcher);
    vi.stubGlobal("chrome", {
      runtime: {
        id: "fixture-id",
        getURL: () => "chrome-extension://fixture-id/",
        onMessage: { addListener: listener },
      },
      storage: { local: storage },
    });
    await import("../../src/background/service-worker");
    const handle = listener.mock.calls[0]?.[0] as (
      message: Message,
      sender: chrome.runtime.MessageSender,
      respond: (value: unknown) => void,
    ) => boolean;
    for (const sender of [
      { id: "fixture-id", url: "https://foreign.example/" },
      {
        id: "other-id",
        url: "chrome-extension://fixture-id/src/popup/index.html",
      },
      {
        id: "fixture-id",
        url: "chrome-extension://fixture-id/src/popup/index.html",
        tab: { id: 1 },
      },
      {
        id: "fixture-id",
        url: "chrome-extension://fixture-id.evil/src/popup/index.html",
      },
    ]) {
      const reply = vi.fn();
      expect(
        handle(
          { type: "GET_STATUS" },
          sender as chrome.runtime.MessageSender,
          reply,
        ),
      ).toBe(false);
      expect(reply).toHaveBeenCalledWith({
        success: false,
        error: "Only extension UI requests are allowed.",
      });
    }
    expect(storage.get).not.toHaveBeenCalled();
    expect(fetcher).not.toHaveBeenCalled();
    const reply = vi.fn();
    expect(
      handle(
        { type: "GET_STATUS" },
        {
          id: "fixture-id",
          url: "chrome-extension://fixture-id/src/popup/index.html",
        },
        reply,
      ),
    ).toBe(true);
    await vi.waitFor(() =>
      expect(reply).toHaveBeenCalledWith({ success: true, data: idle }),
    );
    expect(storage.get).toHaveBeenCalledExactlyOnceWith(DAEMON_SETTINGS_KEY);
    expect(storage.set).not.toHaveBeenCalled();
  });

  it.each([
    ["PAUSE_AUTOMATION", "pause", "submitting", true],
    ["RESUME_AUTOMATION", "resume", "paused", false],
  ] as const)(
    "proxies %s from extension UI to Rust without admission, storage writes or a local engine",
    async (type, action, state, pauseRequested) => {
      vi.resetModules();
      const listener = vi.fn();
      const storage = {
        get: vi.fn().mockResolvedValue({ [DAEMON_SETTINGS_KEY]: settings }),
        set: vi.fn(),
      };
      const progress = { ...idle, state, pauseRequested };
      const fetcher = vi
        .fn<typeof fetch>()
        .mockResolvedValue(new Response(JSON.stringify(progress)));
      vi.stubGlobal("fetch", fetcher);
      vi.stubGlobal("chrome", {
        runtime: {
          id: "fixture-id",
          getURL: () => "chrome-extension://fixture-id/",
          onMessage: { addListener: listener },
        },
        storage: { local: storage },
      });
      await import("../../src/background/service-worker");
      const handle = listener.mock.calls[0]?.[0];
      const reply = vi.fn();
      expect(
        handle(
          { type },
          {
            id: "fixture-id",
            url: "chrome-extension://fixture-id/src/popup/index.html",
          },
          reply,
        ),
      ).toBe(true);
      await vi.waitFor(() =>
        expect(reply).toHaveBeenCalledExactlyOnceWith({
          success: true,
          data: progress,
        }),
      );
      expect(fetcher).toHaveBeenCalledExactlyOnceWith(
        `http://127.0.0.1:8765/batch/${action}`,
        expect.objectContaining({
          method: "POST",
          headers: {
            Authorization: `Bearer ${settings.token}`,
            "Content-Type": "application/json",
          },
          body: "{}",
        }),
      );
      expect(storage.get).toHaveBeenCalledExactlyOnceWith(DAEMON_SETTINGS_KEY);
      expect(storage.set).not.toHaveBeenCalled();
    },
  );

  it.each(["PAUSE_AUTOMATION", "RESUME_AUTOMATION"] as const)(
    "returns %s failures once, never retrying or admitting a batch",
    async (type) => {
      for (const reply of [
        new Response(JSON.stringify({ error: "admission has no controller" }), {
          status: 409,
        }),
        new Response("invalid json"),
      ]) {
        const fetcher = vi.fn<typeof fetch>().mockResolvedValue(reply);
        vi.stubGlobal("fetch", fetcher);
        const result = await proxyDaemonMessage({ type }, async () => settings);
        expect(result).toMatchObject({
          success: false,
          error: expect.any(String),
        });
        expect(fetcher).toHaveBeenCalledOnce();
      }
      const fetcher = vi
        .fn<typeof fetch>()
        .mockRejectedValue(new TypeError("offline"));
      vi.stubGlobal("fetch", fetcher);
      expect(
        await proxyDaemonMessage({ type }, async () => settings),
      ).toMatchObject({
        success: false,
        error: expect.stringContaining("Outcome unknown"),
      });
      expect(fetcher).toHaveBeenCalledOnce();
    },
  );

  it("maps typed trainee refs and session values, returns admission instead of a local report", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
      new Response(JSON.stringify({ batchId: "fixture-admission" }), {
        status: 202,
      }),
    );
    vi.stubGlobal("fetch", fetcher);
    expect(
      await proxyDaemonMessage(
        {
          type: "START_AUTOMATION",
          traineeIds: [],
          trainees: [{ id: "fixture-id" }],
          session: {
            trainingDate: "2026-09-14",
            instructorId: "i",
            trainingTypeId: "t",
          },
        },
        async () => settings,
      ),
    ).toEqual({ success: true, data: { batchId: "fixture-admission" } });
    expect(JSON.parse(String(fetcher.mock.calls[0]?.[1]?.body))).toEqual({
      trainees: [{ id: "fixture-id" }],
      session: {
        training_date: "2026-09-14",
        instructor: "i",
        training_type: "t",
      },
      resume: false,
    });
  });

  it("returns failure on daemon errors with no stored report or TypeScript engine fallback", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockRejectedValue(new TypeError("offline"));
    vi.stubGlobal("fetch", fetcher);
    expect(
      await proxyDaemonMessage({ type: "GET_REPORT" }, async () => settings),
    ).toEqual({
      success: false,
      error: "Daemon disconnected. Start it manually in the rust directory.",
    });
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(
      await proxyDaemonMessage(
        null as unknown as Message,
        async () => settings,
      ),
    ).toEqual({ success: false, error: "Unsupported daemon request." });
    for (const path of [
      "../../src/background/service-worker.ts",
      "../../src/popup/main.ts",
      "../../src/popup/daemonView.ts",
      "../../src/core/infrastructure/daemon/DaemonClient.ts",
    ]) {
      const source = readFileSync(new URL(path, import.meta.url), "utf8");
      expect(source).not.toMatch(
        /import\s+.*(?:AutomationEngine|BatchRunner|RemotePortalAdapter|TrainingLogger|BatchCheckpoint|TaskQueue)/,
      );
      expect(source).not.toMatch(/dssp\.(?:checkpoint|lastReport)/);
    }
  });
});
