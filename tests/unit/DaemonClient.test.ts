import { describe, expect, it, vi } from "vitest";
import {
  DaemonClient,
  DaemonError,
  extensionOrigin,
  isDaemonProgress,
  proxyDaemonMessage,
  startupCommand,
} from "../../src/core/infrastructure/daemon/DaemonClient";
import type { DaemonSettings } from "../../src/core/infrastructure/daemon/DaemonClient";

const settings: DaemonSettings = { port: 8765, token: "a".repeat(64) };
const progress = {
  state: "submitting",
  total: 2,
  processed: 1,
  successful: 1,
  failed: 0,
  skipped: 0,
  indeterminate: 0,
  remaining: 1,
  currentTraineeName: "Fixture B",
};
const batch = {
  trainees: [{ id: "fixture-1" }, { name: "Fixture B" }],
  session: {
    trainingDate: "2026-09-14",
    instructor: "teacher-value",
    trainingType: "practical-value",
  },
};
function response(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status });
}

describe("authenticated loopback daemon client", () => {
  it("uses exact 127.0.0.1 routes, bearer auth, no cookies/cache/redirects and browser-owned Origin", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(response(progress));
    const client = new DaemonClient({ ...settings, port: 9001 }, fetcher);
    expect(await client.status()).toEqual(progress);
    expect(fetcher).toHaveBeenCalledWith("http://127.0.0.1:9001/status", {
      method: "GET",
      headers: { Authorization: `Bearer ${settings.token}` },
      credentials: "omit",
      redirect: "error",
      cache: "no-store",
      referrerPolicy: "no-referrer",
      signal: expect.any(AbortSignal),
    });
    expect(fetcher.mock.calls[0]?.[1]?.headers).not.toHaveProperty("Origin");
  });

  it("reads direct JSON from every GET route including null report/checkpoint", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response(progress))
      .mockResolvedValueOnce(response(null))
      .mockResolvedValueOnce(response(null))
      .mockResolvedValueOnce(
        response({
          instructors: [{ value: "i", label: "Instructor" }],
          trainingTypes: [],
        }),
      );
    const client = new DaemonClient(settings, fetcher);
    await client.status();
    expect(await client.report()).toBeNull();
    expect(await client.checkpoint()).toBeNull();
    expect(await client.formOptions()).toEqual({
      instructors: [{ value: "i", label: "Instructor" }],
      trainingTypes: [],
    });
    expect(fetcher.mock.calls.map(([url]) => url)).toEqual(
      ["status", "report", "checkpoint", "form-options"].map(
        (route) => `http://127.0.0.1:8765/${route}`,
      ),
    );
  });

  it("maps batch fields to snake_case, preserves id/name choice and defaults resume explicitly false", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValue(response({ batchId: "admission-id" }, 202));
    expect(await new DaemonClient(settings, fetcher).start(batch)).toEqual({
      batchId: "admission-id",
    });
    const [url, init] = fetcher.mock.calls[0]!;
    expect(url).toBe("http://127.0.0.1:8765/batch");
    expect(init?.method).toBe("POST");
    expect(init?.headers).toEqual({
      Authorization: `Bearer ${settings.token}`,
      "Content-Type": "application/json",
    });
    expect(JSON.parse(String(init?.body))).toEqual({
      trainees: [{ id: "fixture-1" }, { name: "Fixture B" }],
      session: {
        training_date: "2026-09-14",
        instructor: "teacher-value",
        training_type: "practical-value",
      },
      resume: false,
    });
  });

  it("requires a boolean acknowledgement and does not inherit ambient resume", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValue(response({ batchId: "fixture" }, 202));
    const client = new DaemonClient(settings, fetcher);
    await client.start({ ...batch, resume: true });
    expect(JSON.parse(String(fetcher.mock.calls[0]?.[1]?.body)).resume).toBe(
      true,
    );
    expect(() =>
      client.start({ ...batch, resume: "true" as unknown as boolean }),
    ).toThrow("resume must be a boolean");
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it.each([
    0,
    65536,
    1.5,
    NaN,
    "https://remote.example",
    "127.0.0.1:8765",
    "8765@remote.example",
  ])("rejects unsafe port %s before any fetch", (port) => {
    const fetcher = vi.fn<typeof fetch>();
    expect(
      () => new DaemonClient({ ...settings, port: port as number }, fetcher),
    ).toThrow("Daemon port");
    expect(fetcher).not.toHaveBeenCalled();
  });
  it.each(["", "short", "a".repeat(64) + "\r\nInjected: x", "z".repeat(64)])(
    "rejects malformed credentials",
    (token) => {
      expect(() => new DaemonClient({ ...settings, token })).toThrow("token");
    },
  );
  it("copies validated settings and ignores attempted remote URL injection", async () => {
    const input = { ...settings, url: "https://remote.example" };
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(response(progress));
    const client = new DaemonClient(input, fetcher);
    input.port = 1;
    input.token = "b".repeat(64);
    await client.status();
    expect(fetcher.mock.calls[0]?.[0]).toBe("http://127.0.0.1:8765/status");
    expect(fetcher.mock.calls[0]?.[1]?.headers).toEqual({
      Authorization: `Bearer ${settings.token}`,
    });
  });

  it.each([401, 403, 405, 409, 415, 422, 500, 502, 503])(
    "surfaces HTTP %s once without retry or engine fallback",
    async (code) => {
      const fetcher = vi
        .fn<typeof fetch>()
        .mockResolvedValue(response({ error: "synthetic refusal" }, code));
      await expect(
        new DaemonClient(settings, fetcher).start(batch),
      ).rejects.toMatchObject({
        status: code,
        message: `Daemon HTTP ${code}: synthetic refusal`,
      });
      expect(fetcher).toHaveBeenCalledTimes(1);
    },
  );
  it("treats network loss during POST as unknown admission, never an automatic re-run", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockRejectedValue(new TypeError("offline"));
    const client = new DaemonClient(settings, fetcher);
    await expect(client.start(batch)).rejects.toThrow("Outcome unknown");
    expect(fetcher).toHaveBeenCalledTimes(1);
    await expect(client.status()).rejects.toThrow("Daemon disconnected");
  });
  it("rejects malformed or enveloped responses", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(new Response("not json"))
      .mockResolvedValueOnce(response({ success: true, data: progress }))
      .mockResolvedValueOnce(response({ ...progress, remaining: -1 }));
    const client = new DaemonClient(settings, fetcher);
    await expect(client.status()).rejects.toBeInstanceOf(DaemonError);
    await expect(client.status()).rejects.toThrow("unexpected response");
    await expect(client.status()).rejects.toThrow("unexpected response");
  });

  it("uses actual extension identity in safe manual startup guidance, never a secret", () => {
    for (const url of [
      "chrome-extension://fixture-id/",
      "moz-extension://fixture-uuid/",
    ]) {
      const origin = extensionOrigin(url);
      expect(startupCommand(origin, 8765)).toBe(
        `DSSP_DAEMON_ORIGINS='${origin}' DSSP_DAEMON_PORT=8765 cargo run --offline -- --daemon`,
      );
      expect(startupCommand(origin, 8765)).not.toContain(settings.token);
    }
    expect(() => extensionOrigin("https://foreign.example/")).toThrow(
      "identity",
    );
    expect(() => startupCommand("chrome-extension://id';evil", 8765)).toThrow(
      "identity",
    );
  });
  it("reserves STOP without fetching credentials or executing anything", async () => {
    const load = vi.fn();
    expect(await proxyDaemonMessage({ type: "STOP_AUTOMATION" }, load)).toEqual(
      {
        success: false,
        error: "Stop is not supported (reserved for Stage 31).",
      },
    );
    expect(load).not.toHaveBeenCalled();
  });

  it.each(["pause", "resume"] as const)(
    "%s sends authenticated JSON {} and returns raw progress on 200, never admission",
    async (action) => {
      const value = { ...progress, pauseRequested: action === "pause" };
      const fetcher = vi.fn<typeof fetch>().mockResolvedValue(response(value));
      expect(await new DaemonClient(settings, fetcher)[action]()).toEqual(
        value,
      );
      expect(fetcher).toHaveBeenCalledExactlyOnceWith(
        `http://127.0.0.1:8765/batch/${action}`,
        {
          method: "POST",
          headers: {
            Authorization: `Bearer ${settings.token}`,
            "Content-Type": "application/json",
          },
          body: "{}",
          credentials: "omit",
          redirect: "error",
          cache: "no-store",
          referrerPolicy: "no-referrer",
        },
      );
      expect(fetcher.mock.calls[0]?.[1]?.headers).not.toHaveProperty("Origin");
    },
  );

  it.each(["pause", "resume"] as const)(
    "%s rejects 202, envelopes, batch IDs, malformed JSON and malformed progress without retry",
    async (action) => {
      for (const reply of [
        response({ ...progress, pauseRequested: true }, 202),
        response({ success: true, data: progress }),
        response({ batchId: "not-a-control-result" }),
        new Response("not json"),
        response({ ...progress, pauseRequested: "true" }),
      ]) {
        const fetcher = vi.fn<typeof fetch>().mockResolvedValue(reply);
        await expect(
          new DaemonClient(settings, fetcher)[action](),
        ).rejects.toBeInstanceOf(DaemonError);
        expect(fetcher).toHaveBeenCalledOnce();
      }
    },
  );

  it("keeps admission at 202 rather than accepting control's 200 status", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValue(response({ batchId: "fixture" }));
    await expect(
      new DaemonClient(settings, fetcher).start(batch),
    ).rejects.toMatchObject({ status: 200 });
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it.each(["pause", "resume"] as const)(
    "%s surfaces every control refusal and unknown network outcome once without retry",
    async (action) => {
      for (const code of [401, 403, 405, 409, 415, 422, 500, 502, 503]) {
        const fetcher = vi
          .fn<typeof fetch>()
          .mockResolvedValue(
            response({ error: "no controllable batch" }, code),
          );
        await expect(
          new DaemonClient(settings, fetcher)[action](),
        ).rejects.toMatchObject({
          status: code,
          message: `Daemon HTTP ${code}: no controllable batch`,
        });
        expect(fetcher).toHaveBeenCalledOnce();
      }
      const fetcher = vi
        .fn<typeof fetch>()
        .mockRejectedValue(new TypeError("offline"));
      await expect(
        new DaemonClient(settings, fetcher)[action](),
      ).rejects.toThrow(
        "Daemon connection lost during control. Outcome unknown; inspect daemon status before another control request. This request was not retried.",
      );
      expect(fetcher).toHaveBeenCalledOnce();
    },
  );

  it("allows repeated explicit controls on the same batch without admission or automatic repeats", async () => {
    const pending = { ...progress, pauseRequested: true };
    const paused = { ...pending, state: "paused" };
    const waking = { ...paused, pauseRequested: false };
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response(pending))
      .mockResolvedValueOnce(response(paused))
      .mockResolvedValueOnce(response(waking))
      .mockResolvedValueOnce(response({ ...progress, pauseRequested: false }));
    const client = new DaemonClient(settings, fetcher);
    expect(await client.pause()).toEqual(pending);
    expect(await client.pause()).toEqual(paused);
    expect(await client.resume()).toEqual(waking);
    expect(await client.resume()).toEqual({
      ...progress,
      pauseRequested: false,
    });
    expect(fetcher.mock.calls.map(([url]) => url)).toEqual(
      ["pause", "pause", "resume", "resume"].map(
        (action) => `http://127.0.0.1:8765/batch/${action}`,
      ),
    );
    expect(fetcher.mock.calls.every(([, init]) => init?.body === "{}")).toBe(
      true,
    );
  });

  it("parses paused, pending and waking states without inferring pause from intent", () => {
    for (const state of [
      "paused",
      "submitting",
      "loading_trainee",
      "retrying",
    ]) {
      for (const pauseRequested of [true, false, undefined])
        expect(isDaemonProgress({ ...progress, state, pauseRequested })).toBe(
          true,
        );
    }
    for (const pauseRequested of [null, 0, 1, "false", {}])
      expect(
        isDaemonProgress({ ...progress, state: "paused", pauseRequested }),
      ).toBe(false);
    expect(isDaemonProgress({ ...progress, state: "pause_pending" })).toBe(
      false,
    );
  });
});
