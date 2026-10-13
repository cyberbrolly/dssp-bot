import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { DAEMON_SETTINGS_KEY } from "../../src/core/infrastructure/daemon/DaemonClient";

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
const token = "a".repeat(64);
async function boot(configured = true, admitted = false) {
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
  const fetcher = vi
    .fn<typeof fetch>()
    .mockImplementation(async (url, init) => {
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
        ? idle
        : String(url).endsWith("/form-options")
          ? {
              instructors: [{ value: "i", label: "Fixture Instructor" }],
              trainingTypes: [{ value: "t", label: "Fixture Type" }],
            }
          : null;
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
    await vi.waitFor(() => expect(get("state").textContent).toBe("idle"));
  return { get, storage, fetcher, interval };
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("Stage 29 popup wiring", () => {
  it("polls only read-only state at startup and keeps pause/resume/stop reserved", async () => {
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
      expect(get(id).listeners.click).toBeUndefined();
    }
  });

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
