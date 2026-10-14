import type {
  BatchProgress,
  Message,
  MessageResponse,
} from "../messaging/Messages";
import type { TrainingFormOptions } from "../../domain/TrainingFormOptions";

export interface DaemonResult {
  traineeId: string;
  traineeName: string;
  outcome: "success" | "failed" | "skipped" | "indeterminate";
  attempts: number;
  errorCode?: string;
  message?: string;
}

export const DEFAULT_DAEMON_PORT = 8765;
export const DAEMON_SETTINGS_KEY = "dssp.daemonSettings";

export interface DaemonSettings {
  port: number;
  token: string;
}

// Wire types are deliberately separate from the retired TypeScript engine's state.
export interface DaemonCheckpoint {
  status: "running" | "paused" | "finished" | "interrupted" | "aborted";
  startedAt: string;
  updatedAt: string;
  total: number;
  results: DaemonResult[];
  pending: string[];
  inFlight: string | null;
}

export interface DaemonReport {
  total: number;
  successful: number;
  failed: number;
  skipped: number;
  indeterminate: number;
  successRate: number;
  aborted: boolean;
  startedAt: string;
  finishedAt: string;
  results: DaemonResult[];
}

export type TraineeRef = { id: string } | { name: string };
export interface DaemonBatchInput {
  trainees: TraineeRef[];
  session: { trainingDate: string; instructor: string; trainingType: string };
  resume?: boolean;
}

export class DaemonError extends Error {
  readonly status: number | undefined;
  constructor(message: string, status?: number) {
    super(message);
    this.name = "DaemonError";
    this.status = status;
  }
}

export function validateSettings(value: DaemonSettings): DaemonSettings {
  if (!Number.isInteger(value.port) || value.port < 1 || value.port > 65535) {
    throw new DaemonError("Daemon port must be an integer from 1 to 65535.");
  }
  if (!/^[a-fA-F0-9]{64}$/.test(value.token)) {
    throw new DaemonError(
      "Paste the daemon's 64-character hex token manually.",
    );
  }
  return { port: value.port, token: value.token };
}

export function extensionOrigin(runtimeUrl: string): string {
  const match = /^(chrome-extension|moz-extension):\/\/([a-zA-Z0-9-]+)\/$/.exec(
    runtimeUrl,
  );
  if (!match) throw new DaemonError("Invalid extension identity.");
  return runtimeUrl.slice(0, -1);
}

export function startupCommand(origin: string, port: number): string {
  extensionOrigin(`${origin}/`);
  validateSettings({ port, token: "0".repeat(64) });
  return `DSSP_DAEMON_ORIGINS='${origin}' DSSP_DAEMON_PORT=${port} cargo run --offline -- --daemon`;
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function counts(value: Record<string, unknown>, keys: string[]): boolean {
  return keys.every(
    (key) =>
      typeof value[key] === "number" &&
      Number.isSafeInteger(value[key]) &&
      (value[key] as number) >= 0,
  );
}
function results(value: unknown): boolean {
  return (
    Array.isArray(value) &&
    value.every(
      (row: unknown) =>
        object(row) &&
        typeof row.traineeId === "string" &&
        typeof row.traineeName === "string" &&
        ["success", "failed", "skipped", "indeterminate"].includes(
          String(row.outcome),
        ) &&
        counts(row, ["attempts"]),
    )
  );
}
const countKeys = ["total", "successful", "failed", "skipped", "indeterminate"];
export function isDaemonProgress(value: unknown): value is BatchProgress {
  return (
    object(value) &&
    [
      "idle",
      "initializing",
      "loading_trainee",
      "submitting",
      "retrying",
      "paused",
      "stopped",
      "complete",
    ].includes(String(value.state)) &&
    counts(value, [...countKeys, "processed", "remaining"]) &&
    (value.currentTraineeName === undefined ||
      typeof value.currentTraineeName === "string") &&
    (value.pauseRequested === undefined ||
      typeof value.pauseRequested === "boolean")
  );
}
function isReport(value: unknown): value is DaemonReport | null {
  return (
    value === null ||
    (object(value) &&
      counts(value, countKeys) &&
      typeof value.aborted === "boolean" &&
      typeof value.successRate === "number" &&
      Number.isFinite(value.successRate) &&
      value.successRate >= 0 &&
      value.successRate <= 1 &&
      typeof value.startedAt === "string" &&
      typeof value.finishedAt === "string" &&
      results(value.results))
  );
}
function isCheckpoint(value: unknown): value is DaemonCheckpoint | null {
  return (
    value === null ||
    (object(value) &&
      ["running", "paused", "finished", "interrupted", "aborted"].includes(
        String(value.status),
      ) &&
      counts(value, ["total"]) &&
      typeof value.startedAt === "string" &&
      typeof value.updatedAt === "string" &&
      results(value.results) &&
      Array.isArray(value.pending) &&
      value.pending.every((id: unknown) => typeof id === "string") &&
      (value.inFlight === null || typeof value.inFlight === "string"))
  );
}
function isOptions(value: unknown): value is TrainingFormOptions {
  return (
    object(value) &&
    ["instructors", "trainingTypes"].every(
      (key) =>
        Array.isArray(value[key]) &&
        value[key].every(
          (option: unknown) =>
            object(option) &&
            typeof option.value === "string" &&
            typeof option.label === "string",
        ),
    )
  );
}

export class DaemonClient {
  private readonly settings: DaemonSettings;
  private readonly fetcher: typeof fetch;
  constructor(settings: DaemonSettings, fetcher: typeof fetch = fetch) {
    this.settings = validateSettings(settings);
    this.fetcher = fetcher;
  }

  private async request<T>(
    path: string,
    validate: (value: unknown) => value is T,
    body?: unknown,
    expectedStatus = 200,
  ): Promise<T> {
    let response: Response;
    try {
      // No caller-supplied URL, cookies, redirects, discovery, retries or engine fallback.
      // Origin is browser-owned; never spoof an allowlisted extension identity.
      response = await this.fetcher(
        `http://127.0.0.1:${this.settings.port}${path}`,
        {
          method: body === undefined ? "GET" : "POST",
          headers: {
            Authorization: `Bearer ${this.settings.token}`,
            ...(body === undefined
              ? {}
              : { "Content-Type": "application/json" }),
          },
          ...(body === undefined ? {} : { body: JSON.stringify(body) }),
          credentials: "omit",
          redirect: "error",
          cache: "no-store",
          referrerPolicy: "no-referrer",
          // Read-only polling must not hang forever; worker admission can wait for login.
          ...(body === undefined && path !== "/form-options"
            ? { signal: AbortSignal.timeout(5000) }
            : {}),
        },
      );
    } catch {
      throw new DaemonError(
        body === undefined
          ? "Daemon disconnected. Start it manually in the rust directory."
          : path === "/batch"
            ? "Daemon connection lost during admission. Outcome unknown; inspect daemon status/checkpoint before another start. This request was not retried."
            : "Daemon connection lost during control. Outcome unknown; inspect daemon status before another control request. This request was not retried.",
      );
    }
    let value: unknown;
    try {
      value = await response.json();
    } catch {
      throw new DaemonError(
        `Daemon returned invalid JSON (HTTP ${response.status}).`,
        response.status,
      );
    }
    if (!response.ok) {
      const detail =
        object(value) && typeof value.error === "string"
          ? value.error
          : "Request failed.";
      throw new DaemonError(
        `Daemon HTTP ${response.status}: ${detail}`,
        response.status,
      );
    }
    if (response.status !== expectedStatus || !validate(value)) {
      throw new DaemonError(
        "Daemon returned an unexpected response.",
        response.status,
      );
    }
    return value;
  }

  status(): Promise<BatchProgress> {
    return this.request("/status", isDaemonProgress);
  }
  report(): Promise<DaemonReport | null> {
    return this.request("/report", isReport);
  }
  checkpoint(): Promise<DaemonCheckpoint | null> {
    return this.request("/checkpoint", isCheckpoint);
  }
  formOptions(): Promise<TrainingFormOptions> {
    return this.request("/form-options", isOptions);
  }
  pause(): Promise<BatchProgress> {
    return this.request("/batch/pause", isDaemonProgress, {});
  }
  resume(): Promise<BatchProgress> {
    return this.request("/batch/resume", isDaemonProgress, {});
  }
  start(input: DaemonBatchInput): Promise<{ batchId: string }> {
    if (input.resume !== undefined && typeof input.resume !== "boolean")
      throw new DaemonError("resume must be a boolean.");
    return this.request(
      "/batch",
      (value): value is { batchId: string } =>
        object(value) && typeof value.batchId === "string",
      {
        trainees: input.trainees.map((trainee) =>
          "id" in trainee ? { id: trainee.id } : { name: trainee.name },
        ),
        session: {
          training_date: input.session.trainingDate,
          instructor: input.session.instructor,
          training_type: input.session.trainingType,
        },
        resume: input.resume ?? false,
      },
      202,
    );
  }
}

export async function proxyDaemonMessage(
  message: Message,
  settings: () => Promise<DaemonSettings>,
): Promise<MessageResponse> {
  if (!message || typeof message.type !== "string")
    return { success: false, error: "Unsupported daemon request." };
  if (message.type === "STOP_AUTOMATION") {
    return {
      success: false,
      error: "Stop is not supported (reserved for Stage 31).",
    };
  }
  try {
    const client = new DaemonClient(await settings());
    let data: unknown;
    switch (message.type) {
      case "GET_STATUS":
        data = await client.status();
        break;
      case "PAUSE_AUTOMATION":
        data = await client.pause();
        break;
      case "RESUME_AUTOMATION":
        data = await client.resume();
        break;
      case "GET_REPORT":
        data = await client.report();
        break;
      case "GET_CHECKPOINT":
        data = await client.checkpoint();
        break;
      case "GET_FORM_OPTIONS":
        data = await client.formOptions();
        break;
      case "START_AUTOMATION":
        data = await client.start({
          trainees:
            message.trainees ?? message.traineeIds.map((name) => ({ name })),
          session: {
            trainingDate: message.session.trainingDate,
            instructor: message.session.instructorId,
            trainingType: message.session.trainingTypeId,
          },
          resume: message.resume ?? false,
        });
        break;
      default:
        return { success: false, error: "Unsupported daemon request." };
    }
    return { success: true, data };
  } catch (error) {
    return {
      success: false,
      error: error instanceof Error ? error.message : "Daemon request failed.",
    };
  }
}
