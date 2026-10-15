import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { DAEMON_SETTINGS_KEY } from "../../src/core/infrastructure/daemon/DaemonClient";
import type {
  DaemonCheckpoint,
  DaemonReport,
} from "../../src/core/infrastructure/daemon/DaemonClient";
import type { BatchProgress } from "../../src/core/infrastructure/messaging/Messages";

// Minimal DOM fixture: no new dependency and no browser/portal requests.
class ElementFixture {
  value = "";
  textContent: string | null = "";
  disabled = false;
  hidden = false;
  dataset: Record<string, string> = {};
  children: ElementFixture[] = [];
  listeners: Record<string, () => void> = {};
  addEventListener(event: string, handler: () => void): void {
    this.listeners[event] = handler;
  }
  replaceChildren(): void {
    this.children = [];
    this.value = "";
  }
  append(child: ElementFixture): void {
    this.children.push(child);
  }
  click(): void {
    if (!this.disabled) this.listeners.click?.();
  }
}
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
const token = "a".repeat(64);
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const active: BatchProgress = {
  ...idle,
  state: "submitting",
  total: 2,
  processed: 1,
  successful: 1,
  remaining: 1,
  currentTraineeName: "Fixture B",
  pauseRequested: false,
};
const pausedCheckpoint: DaemonCheckpoint = {
  status: "paused",
  startedAt: "1789382400000",
  updatedAt: "1789382401000",
  total: 2,
  results: [
    {
      traineeId: "a",
      traineeName: "Fixture A",
      outcome: "success",
      attempts: 1,
    },
  ],
  pending: ["b"],
  inFlight: null,
};
function response(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status });
}
async function boot(
  configured = true,
  admitted = false,
  initialProgress = idle,
) {
  vi.resetModules();
  const elements = new Map<string, ElementFixture>();
  const get = (id: string): ElementFixture => {
    if (!elements.has(id)) elements.set(id, new ElementFixture());
    return elements.get(id)!;
  };
  const storage = {
    get: vi
      .fn()
      .mockResolvedValue(
        configured ? { [DAEMON_SETTINGS_KEY]: { port: 8765, token } } : {},
      ),
    set: vi.fn().mockResolvedValue(undefined),
  };
  const daemon = {
    progress: initialProgress,
    checkpoint: null as DaemonCheckpoint | null,
    report: null as DaemonReport | null,
  };
  const fetcher = vi
    .fn<typeof fetch>()
    .mockImplementation(async (url, init) => {
      if (
        String(url).endsWith("/batch/pause") ||
        String(url).endsWith("/batch/resume") ||
        String(url).endsWith("/batch/stop")
      )
        return response(daemon.progress);
      if (init?.method === "POST")
        return new Response(
          JSON.stringify(
            admitted
              ? { batchId: "fixture-admission" }
              : { error: "unfinished predecessor; human review required" },
          ),
          { status: admitted ? 202 : 409 },
        );
      const value = String(url).endsWith("/status")
        ? daemon.progress
        : String(url).endsWith("/checkpoint")
          ? daemon.checkpoint
          : String(url).endsWith("/form-options")
            ? {
                instructors: [{ value: "i", label: "Fixture Instructor" }],
                trainingTypes: [{ value: "t", label: "Fixture Type" }],
              }
            : daemon.report;
      return new Response(JSON.stringify(value));
    });
  const interval = vi.fn().mockReturnValue(1);
  vi.stubGlobal("fetch", fetcher);
  vi.stubGlobal("chrome", {
    runtime: { getURL: () => "chrome-extension://fixture-id/" },
    storage: { local: storage },
  });
  vi.stubGlobal("document", {
    getElementById: get,
    createElement: () => new ElementFixture(),
  });
  vi.stubGlobal("window", {
    setInterval: interval,
    clearInterval: vi.fn(),
    addEventListener: vi.fn(),
  });
  await import("../../src/popup/main");
  await vi.waitFor(() => expect(storage.get).toHaveBeenCalledOnce());
  if (configured)
    await vi.waitFor(() =>
      expect(get("state").textContent).toBe(
        initialProgress.state.replace(/_/g, " "),
      ),
    );
  const poll = () => interval.mock.calls[0]?.[0]();
  const posts = () =>
    fetcher.mock.calls.filter(([, init]) => init?.method === "POST");
  return { get, storage, fetcher, interval, daemon, poll, posts };
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Stages 30–31 popup wiring (offline DOM fixture)", () => {
  it("polls only read-only state at startup; wires controls but keeps them disabled while idle", async () => {
    const { get, fetcher, storage } = await boot();
    expect(fetcher.mock.calls.map(([url]) => String(url))).toEqual([
      "http://127.0.0.1:8765/status",
      "http://127.0.0.1:8765/report",
      "http://127.0.0.1:8765/checkpoint",
    ]);
    expect(storage.set).not.toHaveBeenCalled();
    expect(get("start").disabled).toBe(true);
    const html = readFileSync(
      new URL("../../src/popup/index.html", import.meta.url),
      "utf8",
    );
    expect(html).toMatch(/id="daemon-token"\s+type="password"/);
    for (const id of ["pause", "resume", "stop"]) {
      expect(html).toMatch(new RegExp(`id="${id}"[^>]*disabled`));
      expect(get(id).disabled).toBe(true);
      get(id).click();
      expect(get(id).listeners.click).toBeTypeOf("function");
    }
  });

  it("renders pending pause, persisted pause and same-batch resume only from polling, with no local execution or storage", async () => {
    const { get, storage, fetcher, daemon, poll, posts } = await boot(
      true,
      false,
      active,
    );
    const read = fetcher.getMockImplementation()!;
    const pause = deferred<Response>();
    fetcher.mockImplementation((url, init) =>
      String(url).endsWith("/batch/pause") ? pause.promise : read(url, init),
    );
    get("pause").click();
    get("pause").click();
    expect(get("state").textContent).toBe("submitting");
    expect(get("pause-status").hidden).toBe(true);
    for (const id of [
      "pause",
      "resume",
      "start",
      "load-options",
      "save-daemon",
      "instructor",
      "training-type",
      "training-date",
      "trainees",
    ])
      expect(get(id).disabled).toBe(true);
    daemon.progress = { ...active, pauseRequested: true };
    // The response says paused, but only the subsequent GETs may render it.
    pause.resolve(
      response({ ...active, state: "paused", pauseRequested: true }),
    );
    await vi.waitFor(() =>
      expect(get("pause-status").textContent).toContain("Pause pending"),
    );
    expect(get("state").textContent).toBe("submitting");
    expect(get("current").textContent).toBe("Fixture B");
    expect(get("processed").textContent).toBe("1");
    expect(get("pause").disabled).toBe(true);
    expect(get("resume").disabled).toBe(false);
    for (const id of [
      "start",
      "load-options",
      "instructor",
      "training-type",
      "training-date",
      "trainees",
    ])
      expect(get(id).disabled).toBe(true);

    daemon.progress = {
      ...active,
      state: "paused",
      currentTraineeName: undefined,
      pauseRequested: true,
    };
    daemon.checkpoint = pausedCheckpoint;
    poll();
    await vi.waitFor(() => expect(get("state").textContent).toBe("paused"));
    expect(get("pause-status").textContent).toContain(
      "checkpoint was persisted",
    );
    expect(get("recovery").textContent).toContain("not an interrupted batch");
    const resume = deferred<Response>();
    fetcher.mockImplementation((url, init) =>
      String(url).endsWith("/batch/resume") ? resume.promise : read(url, init),
    );
    get("resume").click();
    get("resume").click();
    expect(get("resume").disabled).toBe(true);
    expect(get("save-daemon").disabled).toBe(true);
    expect(get("state").textContent).toBe("paused");
    daemon.progress = { ...daemon.progress, pauseRequested: false };
    resume.resolve(response({ ...active, state: "complete" }));
    await vi.waitFor(() =>
      expect(get("pause-status").textContent).toContain(
        "waiting for Rust to persist the running checkpoint",
      ),
    );
    expect(get("state").textContent).toBe("paused");
    expect(get("pause").disabled).toBe(true);
    expect(get("resume").disabled).toBe(true);
    expect(get("training-date").disabled).toBe(true);

    daemon.progress = { ...active, state: "loading_trainee" };
    poll();
    await vi.waitFor(() =>
      expect(get("state").textContent).toBe("loading trainee"),
    );
    expect(get("pause-status").hidden).toBe(true);
    expect(get("pause").disabled).toBe(false);
    expect(get("resume").disabled).toBe(true);
    expect(posts().map(([url]) => String(url))).toEqual([
      "http://127.0.0.1:8765/batch/pause",
      "http://127.0.0.1:8765/batch/resume",
    ]);
    expect(posts().every(([, init]) => init?.body === "{}")).toBe(true);
    expect(storage.set).not.toHaveBeenCalled();
    expect(get("stop").disabled).toBe(false);
  });

  it("resume cancels pending pause without new admission or a local state transition", async () => {
    const { get, fetcher, daemon, posts } = await boot(true, false, {
      ...active,
      pauseRequested: true,
    });
    const read = fetcher.getMockImplementation()!;
    fetcher.mockImplementation((url, init) => {
      if (init?.method === "POST") {
        daemon.progress = active;
        return Promise.resolve(
          response({ ...active, state: "paused", pauseRequested: false }),
        );
      }
      return read(url, init);
    });
    get("resume").click();
    expect(get("state").textContent).toBe("submitting");
    await vi.waitFor(() => expect(get("pause").disabled).toBe(false));
    expect(get("pause-status").hidden).toBe(true);
    expect(get("state").textContent).toBe("submitting");
    expect(posts()).toHaveLength(1);
    expect(posts()[0]?.[0]).toBe("http://127.0.0.1:8765/batch/resume");
  });

  it("reconnects to the daemon's paused batch without controls or recovery POSTs, and distinguishes a restart", async () => {
    const { get, fetcher, daemon, poll, posts, storage } = await boot(
      true,
      false,
      { ...active, state: "paused", pauseRequested: true },
    );
    const read = fetcher.getMockImplementation()!;
    fetcher.mockRejectedValue(new TypeError("offline"));
    poll();
    await vi.waitFor(() =>
      expect(get("state").textContent).toBe("disconnected"),
    );
    expect(get("pause-status").hidden).toBe(true);
    for (const id of [
      "pause",
      "resume",
      "start",
      "load-options",
      "training-date",
      "trainees",
    ])
      expect(get(id).disabled).toBe(true);
    fetcher.mockImplementation(read);
    daemon.checkpoint = pausedCheckpoint;
    poll();
    await vi.waitFor(() => expect(get("state").textContent).toBe("paused"));
    expect(get("resume").disabled).toBe(false);
    expect(get("recovery").textContent).toContain("in-memory paused batch");
    daemon.progress = idle;
    poll();
    await vi.waitFor(() => expect(get("state").textContent).toBe("idle"));
    expect(get("resume").disabled).toBe(true);
    expect(get("recovery").textContent).toContain(
      "durable checkpoint alone is not a resumable in-memory batch",
    );
    expect(get("recovery").textContent).toContain("resume: true");
    expect(posts()).toHaveLength(0);
    expect(storage.set).not.toHaveBeenCalled();
  });

  it.each([
    ["pause", "late idle"],
    ["resume", "late idle"],
    ["pause", "late error"],
    ["resume", "late error"],
    ["stop", "late idle"],
    ["stop", "late active"],
    ["stop", "late error"],
  ] as const)(
    "ignores stale %s reads (%s), even when a pre-result read is still in flight",
    async (action, outcome) => {
      const initial =
        action !== "resume"
          ? active
          : { ...active, state: "paused" as const, pauseRequested: true };
      const { get, fetcher, daemon, poll, posts } = await boot(
        true,
        false,
        initial,
      );
      const read = fetcher.getMockImplementation()!;
      const stale = deferred<Response>();
      fetcher.mockImplementationOnce(() => stale.promise);
      poll(); // Hold GET /status while report/checkpoint finish.
      daemon.progress =
        action === "pause"
          ? { ...active, state: "paused", pauseRequested: true }
          : action === "stop"
            ? { ...active, state: "retrying", stopRequested: true }
            : active;
      get(action).click();
      // A fresh generation must poll without waiting for the stale request.
      await vi.waitFor(() =>
        expect(get("state").textContent).toBe(
          daemon.progress.state.replace(/_/g, " "),
        ),
      );
      const before = get("pause-status").textContent;
      if (outcome === "late error")
        stale.reject(new TypeError("old connection lost"));
      else stale.resolve(response(outcome === "late active" ? active : idle));
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      expect(get("state").textContent).toBe(
        daemon.progress.state.replace(/_/g, " "),
      );
      expect(get("pause-status").textContent).toBe(before);
      expect(get("training-date").disabled).toBe(true);
      expect(posts()).toHaveLength(1);
      fetcher.mockImplementation(read);
    },
  );

  it.each(["pause", "resume", "stop"] as const)(
    "does not retry %s on 409, HTTP errors or connection loss; polls authoritative state",
    async (action) => {
      for (const failure of [409, 500, "offline"] as const) {
        const initial =
          action !== "resume"
            ? active
            : { ...active, state: "paused" as const, pauseRequested: true };
        const { get, fetcher, poll, posts } = await boot(true, false, initial);
        const read = fetcher.getMockImplementation()!;
        fetcher.mockImplementation((url, init) =>
          init?.method === "POST"
            ? failure === "offline"
              ? Promise.reject(new TypeError("offline"))
              : Promise.resolve(
                  response({ error: "no controllable batch" }, failure),
                )
            : read(url, init),
        );
        get(action).click();
        await vi.waitFor(() =>
          expect(get("status").dataset.tone).toBe("error"),
        );
        await vi.waitFor(() => expect(get(action).disabled).toBe(false));
        expect(get("status").textContent).toContain(
          failure === "offline" ? "Outcome unknown" : `Daemon HTTP ${failure}`,
        );
        poll();
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
        expect(posts()).toHaveLength(1);
        expect(get("state").textContent).toBe(initial.state.replace(/_/g, " "));
      }
    },
  );

  it.each([
    ["idle", false, false, false],
    ["initializing", false, false, false],
    ["complete", false, false, false],
    ["stopped", false, false, false],
    ["submitting", false, true, false],
    ["loading_trainee", false, true, false],
    ["retrying", false, true, false],
    ["submitting", true, false, true],
    ["paused", true, false, true],
    ["paused", false, false, false],
  ] as const)(
    "enables only valid controls for state=%s pauseRequested=%s",
    async (state, pauseRequested, canPause, canResume) => {
      const { get } = await boot(true, false, {
        ...active,
        state,
        pauseRequested,
      });
      expect(get("pause").disabled).toBe(!canPause);
      expect(get("resume").disabled).toBe(!canResume);
      expect(get("stop").disabled).toBe(
        !["loading_trainee", "submitting", "retrying", "paused"].includes(
          state,
        ),
      );
    },
  );

  it("does not discover credentials or start anything when unconfigured; saves settings only", async () => {
    const { get, fetcher, storage } = await boot(false);
    expect(fetcher).not.toHaveBeenCalled();
    expect(get("daemon-command").textContent).toContain(
      "DSSP_DAEMON_ORIGINS='chrome-extension://fixture-id'",
    );
    expect(get("daemon-command").textContent).not.toContain(token);
    get("daemon-token").value = token;
    get("daemon-port").value = "9100";
    get("save-daemon").click();
    await vi.waitFor(() => expect(get("state").textContent).toBe("idle"));
    expect(storage.set).toHaveBeenCalledExactlyOnceWith({
      [DAEMON_SETTINGS_KEY]: { port: 9100, token },
    });
    expect(
      fetcher.mock.calls.every(([url]) =>
        String(url).startsWith("http://127.0.0.1:9100/"),
      ),
    ).toBe(true);
  });

  it("loads options explicitly and shows a 409 once without automatic resume/retry/TypeScript engine", async () => {
    const { get, fetcher, storage, interval } = await boot();
    get("load-options").click();
    await vi.waitFor(() =>
      expect(get("status").textContent).toBe("Form options loaded."),
    );
    get("instructor").value = "i";
    get("training-type").value = "t";
    get("training-date").value = "2026-09-14";
    get("trainees").value = "Fixture A, Fixture B";
    get("start").click();
    await vi.waitFor(() =>
      expect(get("status").textContent).toContain("Daemon HTTP 409"),
    );
    expect(get("status").textContent).not.toContain("Batch finished.");
    const posts = fetcher.mock.calls.filter(
      ([, init]) => init?.method === "POST",
    );
    expect(posts).toHaveLength(1);
    expect(JSON.parse(String(posts[0]?.[1]?.body))).toEqual({
      trainees: [{ name: "Fixture A" }, { name: "Fixture B" }],
      session: {
        training_date: "2026-09-14",
        instructor: "i",
        training_type: "t",
      },
      resume: false,
    });
    interval.mock.calls[0]?.[0]();
    await vi.waitFor(() =>
      expect(
        fetcher.mock.calls.filter(([, init]) => init?.method === "POST"),
      ).toHaveLength(1),
    );
    expect(storage.set).not.toHaveBeenCalled();
  });

  it("keeps controls and session edits locked during blocked admission until polling reports an installed active batch", async () => {
    const { get, fetcher, daemon, poll, posts } = await boot();
    get("load-options").click();
    await vi.waitFor(() =>
      expect(get("status").textContent).toBe("Form options loaded."),
    );
    await vi.waitFor(() => expect(get("instructor").disabled).toBe(false));
    get("instructor").value = "i";
    get("training-type").value = "t";
    get("training-date").value = "2026-09-14";
    get("trainees").value = "Fixture A, Fixture B";
    const read = fetcher.getMockImplementation()!;
    const admission = deferred<Response>();
    fetcher.mockImplementation((url, init) =>
      init?.method === "POST" ? admission.promise : read(url, init),
    );
    get("start").click();
    daemon.progress = { ...active, state: "initializing" };
    poll();
    await vi.waitFor(() =>
      expect(get("state").textContent).toBe("initializing"),
    );
    for (const id of [
      "start",
      "pause",
      "resume",
      "stop",
      "save-daemon",
      "load-options",
      "instructor",
      "training-type",
      "training-date",
      "trainees",
    ])
      expect(get(id).disabled).toBe(true);
    get("pause").click();
    get("resume").click();
    expect(posts()).toHaveLength(1);
    daemon.progress = active;
    admission.resolve(response({ batchId: "fixture-admission" }, 202));
    await vi.waitFor(() => expect(get("pause").disabled).toBe(false));
    expect(get("state").textContent).toBe("submitting");
    expect(get("resume").disabled).toBe(true);
    expect(get("training-date").disabled).toBe(true);
    expect(posts()).toHaveLength(1);
    expect(posts()[0]?.[0]).toBe("http://127.0.0.1:8765/batch");
  });

  it("renders 202 as admission, not completion; Rust supplies the eventual report", async () => {
    const { get, fetcher } = await boot(true, true);
    get("load-options").click();
    await vi.waitFor(() =>
      expect(get("status").textContent).toBe("Form options loaded."),
    );
    get("instructor").value = "i";
    get("training-type").value = "t";
    get("training-date").value = "2026-09-14";
    get("trainees").value = "Fixture";
    get("start").click();
    get("start").click();
    await vi.waitFor(() =>
      expect(get("status").textContent).toContain(
        "Batch admitted (fixture-admission)",
      ),
    );
    expect(get("report-section").hidden).toBe(true);
    expect(
      fetcher.mock.calls.filter(([, init]) => init?.method === "POST"),
    ).toHaveLength(1);
  });

  it("clears stale displays and locks start on disconnection; shows manual command without exposing the token", async () => {
    const { get, fetcher, interval } = await boot();
    fetcher.mockRejectedValue(new TypeError("offline"));
    interval.mock.calls[0]?.[0]();
    await vi.waitFor(() =>
      expect(get("connection").textContent).toContain("Daemon disconnected"),
    );
    expect(get("state").textContent).toBe("disconnected");
    expect(get("total").textContent).toBe("—");
    expect(get("start").disabled).toBe(true);
    expect(get("daemon-help").hidden).toBe(false);
    expect(get("daemon-command").textContent).not.toContain(token);
  });
});

const sessionControls = [
  "start",
  "load-options",
  "instructor",
  "training-type",
  "training-date",
  "trainees",
] as const;
const batchControls = ["pause", "resume", "stop"] as const;
const successfulReport: DaemonReport = {
  total: 1,
  successful: 1,
  failed: 0,
  skipped: 0,
  indeterminate: 0,
  successRate: 1,
  aborted: false,
  startedAt: "1789382400000",
  finishedAt: "1789382401000",
  results: [
    {
      traineeId: "a",
      traineeName: "Fixture A",
      outcome: "success",
      attempts: 1,
    },
  ],
};

describe("Stage 31 latched stop (offline DOM fixture)", () => {
  it.each([
    ["submitting", false],
    ["submitting", true],
    ["retrying", false],
    ["paused", true],
    ["paused", false],
  ] as const)(
    "stops from %s (pauseRequested=%s), waits for authoritative terminal publication, and retains the terminal lock",
    async (state, pauseRequested) => {
      const { get, fetcher, daemon, poll, posts, storage } = await boot();
      get("load-options").click();
      await vi.waitFor(() => expect(get("start").disabled).toBe(false));
      daemon.progress = { ...active, state, pauseRequested };
      daemon.checkpoint = pausedCheckpoint;
      poll();
      await vi.waitFor(() => expect(get("state").textContent).toBe(state));
      const read = fetcher.getMockImplementation()!;
      const stop = deferred<Response>();
      fetcher.mockImplementation((url, init) =>
        init?.method === "POST" ? stop.promise : read(url, init),
      );
      get("stop").click();
      get("stop").click();
      for (const id of [...sessionControls, ...batchControls, "save-daemon"])
        expect(get(id).disabled).toBe(true);
      expect(get("status").textContent).toContain(
        "does not cancel an in-flight POST",
      );
      expect(get("state").textContent).toBe(state);
      // Even a terminal control body must not publish a local terminal state.
      daemon.progress = {
        ...active,
        state,
        pauseRequested: false,
        stopRequested: true,
      };
      stop.resolve(response({ ...daemon.progress, state: "stopped" }));
      await vi.waitFor(() =>
        expect(get("pause-status").textContent).toContain("Stop pending"),
      );
      expect(get("state").textContent).toBe(state);
      expect(get("pause-status").textContent).not.toContain("Resume continues");
      expect(get("recovery").textContent).not.toContain("resume: true");
      for (const id of [...sessionControls, ...batchControls]) {
        expect(get(id).disabled).toBe(true);
        get(id).click();
      }
      // A dispatched event must not bypass the disabled form-options guard.
      get("load-options").listeners.click?.();
      expect(posts()).toHaveLength(1);
      expect(posts()[0]).toEqual([
        "http://127.0.0.1:8765/batch/stop",
        expect.objectContaining({
          method: "POST",
          body: "{}",
          headers: {
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
          },
        }),
      ]);
      expect(
        fetcher.mock.calls.filter(([url]) =>
          String(url).endsWith("/form-options"),
        ),
      ).toHaveLength(1);
      daemon.progress = {
        ...daemon.progress,
        state: "stopped",
        processed: 2,
        skipped: 1,
        remaining: 0,
      };
      daemon.report = {
        ...successfulReport,
        total: 2,
        skipped: 1,
        successRate: 0.5,
        aborted: true,
        results: [
          ...successfulReport.results,
          {
            traineeId: "b",
            traineeName: "Fixture B",
            outcome: "skipped",
            attempts: 0,
            message: "not submitted: operator stop",
          },
        ],
      };
      daemon.checkpoint = {
        ...pausedCheckpoint,
        status: "aborted",
        pending: [],
        results: daemon.report.results,
      };
      poll();
      await vi.waitFor(() => expect(get("state").textContent).toBe("stopped"));
      expect(get("pause-status").textContent).toContain(
        "Rust reports stopped, not successful completion",
      );
      expect(get("report").textContent).toContain(
        "1/2 recorded, 0 failed, 1 skipped, 50% success rate",
      );
      expect(get("report").textContent).toContain(
        "Batch aborted; human review",
      );
      expect(get("recovery").textContent).toContain(
        "Start and session edits stay locked",
      );
      expect(get("recovery").textContent).not.toContain("resume: true");
      for (const id of [...sessionControls, ...batchControls])
        expect(get(id).disabled).toBe(true);
      expect(storage.set).not.toHaveBeenCalled();
      expect(posts()).toHaveLength(1);
      // Only new authoritative progress, not a control acknowledgement, clears the latch lock.
      daemon.progress = { ...active, stopRequested: false };
      daemon.report = null;
      poll();
      await vi.waitFor(() => expect(get("pause").disabled).toBe(false));
      expect(get("start").disabled).toBe(true);
      daemon.progress = { ...idle, state: "complete", stopRequested: false };
      poll();
      await vi.waitFor(() => expect(get("start").disabled).toBe(false));
      expect(get("training-date").disabled).toBe(false);
      expect(posts()).toHaveLength(1);
    },
  );

  it.each([
    "idle",
    "initializing",
    "loading_trainee",
    "submitting",
    "retrying",
    "paused",
    "stopped",
    "complete",
  ] as const)(
    "never unlocks execution or editing while stopRequested is true, even in %s",
    async (state) => {
      const { get, posts } = await boot(true, false, {
        ...active,
        state,
        stopRequested: true,
        pauseRequested: true,
      });
      for (const id of [...sessionControls, ...batchControls]) {
        expect(get(id).disabled).toBe(true);
        get(id).listeners.click?.();
      }
      expect(posts()).toHaveLength(0);
    },
  );

  it.each(["running", "paused", "finished", "missing"] as const)(
    "shows missing-report human review and preserves the last durable %s checkpoint after stop finalization failure",
    async (durableState) => {
      const { get, daemon, poll, posts } = await boot(true, false, {
        ...active,
        stopRequested: true,
      });
      daemon.checkpoint =
        durableState === "missing"
          ? null
          : {
              ...pausedCheckpoint,
              status: durableState,
              // Retain uncertainty from a prior running write; never clear it locally.
              inFlight: durableState === "running" ? "b" : null,
            };
      const before = JSON.stringify(daemon.checkpoint);
      daemon.progress = { ...active, state: "stopped", stopRequested: true };
      poll();
      await vi.waitFor(() => expect(get("state").textContent).toBe("stopped"));
      expect(get("successful").textContent).toBe("1");
      expect(get("processed").textContent).toBe("1");
      expect(get("remaining").textContent).toBe("1");
      expect(get("report-section").hidden).toBe(true);
      expect(get("report").textContent).toBe("");
      expect(get("recovery-section").hidden).toBe(false);
      expect(get("recovery").textContent).toContain(
        "No final report is available",
      );
      expect(get("recovery").textContent).toContain(
        "checkpoint write or execution failure",
      );
      expect(get("recovery").textContent).toContain("Human review is required");
      expect(get("recovery").textContent).toContain("no automatic recovery");
      expect(get("recovery").textContent).not.toContain("resume: true");
      if (durableState === "running")
        expect(get("recovery").textContent).toContain(
          "b — may have reached the portal",
        );
      for (const id of [...sessionControls, ...batchControls])
        expect(get(id).disabled).toBe(true);
      expect(JSON.stringify(daemon.checkpoint)).toBe(before);
      expect(posts()).toHaveLength(0);
    },
  );

  it.each([true, false])(
    "renders actual last-row success when stop wins=%s, without inventing skips or relabeling completion",
    async (stopWins) => {
      const { get, daemon, fetcher, poll, posts } = await boot(
        true,
        false,
        active,
      );
      const stop = deferred<Response>();
      const read = fetcher.getMockImplementation()!;
      fetcher.mockImplementation((url, init) =>
        init?.method === "POST" ? stop.promise : read(url, init),
      );
      get("stop").click();
      daemon.progress = {
        ...idle,
        state: stopWins ? "stopped" : "complete",
        total: 1,
        processed: 1,
        successful: 1,
        stopRequested: stopWins,
      };
      daemon.report = { ...successfulReport, aborted: stopWins };
      daemon.checkpoint = {
        ...pausedCheckpoint,
        status: stopWins ? "aborted" : "finished",
        total: 1,
        pending: [],
      };
      poll();
      await vi.waitFor(() =>
        expect(get("state").textContent).toBe(
          stopWins ? "stopped" : "complete",
        ),
      );
      // This is stale progress. Only the subsequent fresh reads may update the UI.
      stop.resolve(
        stopWins
          ? response({ ...active, stopRequested: true })
          : response({ error: "batch has no active controller" }, 409),
      );
      await vi.waitFor(() =>
        expect(get("status").textContent).toContain(
          stopWins ? "Stop request accepted" : "Daemon HTTP 409",
        ),
      );
      await vi.waitFor(() => expect(get("save-daemon").disabled).toBe(false));
      expect(get("state").textContent).toBe(stopWins ? "stopped" : "complete");
      expect(get("report-section").hidden).toBe(false);
      expect(get("report").textContent).toContain(
        "1/1 recorded, 0 failed, 0 skipped, 100% success rate",
      );
      expect(get("report").textContent?.includes("Batch aborted")).toBe(
        stopWins,
      );
      expect(get("pause-status").hidden).toBe(!stopWins);
      expect(daemon.report.results[0]?.outcome).toBe("success");
      expect(get("training-date").disabled).toBe(stopWins);
      expect(posts()).toHaveLength(1);
    },
  );

  it.each(["success", "409", "network loss"] as const)(
    "keeps a fresh disconnection authoritative over a late stop response (%s), then reconnects only by polling",
    async (outcome) => {
      const { get, daemon, fetcher, poll, posts, storage } = await boot(
        true,
        false,
        active,
      );
      const read = fetcher.getMockImplementation()!;
      const stop = deferred<Response>();
      let offline = false;
      fetcher.mockImplementation((url, init) =>
        init?.method === "POST"
          ? stop.promise
          : offline
            ? Promise.reject(new TypeError("offline"))
            : read(url, init),
      );
      get("stop").click();
      offline = true;
      poll();
      await vi.waitFor(() =>
        expect(get("state").textContent).toBe("disconnected"),
      );
      if (outcome === "network loss")
        stop.reject(new TypeError("late control disconnect"));
      else
        stop.resolve(
          outcome === "409"
            ? response({ error: "no active controller" }, 409)
            : response({ ...active, stopRequested: false }),
        );
      await vi.waitFor(() => expect(get("save-daemon").disabled).toBe(false));
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
      expect(get("state").textContent).toBe("disconnected");
      expect(get("pause-status").hidden).toBe(true);
      expect(get("report-section").hidden).toBe(true);
      for (const id of [...sessionControls, ...batchControls])
        expect(get(id).disabled).toBe(true);
      offline = false;
      daemon.progress = { ...active, state: "retrying", stopRequested: true };
      poll();
      await vi.waitFor(() => expect(get("state").textContent).toBe("retrying"));
      expect(get("pause-status").textContent).toContain("Stop pending");
      for (const id of [...sessionControls, ...batchControls])
        expect(get(id).disabled).toBe(true);
      expect(posts()).toHaveLength(1);
      expect(storage.set).not.toHaveBeenCalled();
    },
  );
});
