import {
  DAEMON_SETTINGS_KEY,
  DEFAULT_DAEMON_PORT,
  DaemonClient,
  extensionOrigin,
  startupCommand,
  validateSettings,
} from "../core/infrastructure/daemon/DaemonClient";
import type { DaemonSettings } from "../core/infrastructure/daemon/DaemonClient";
import type { TrainingFormOption } from "../core/domain/TrainingFormOptions";
import { parseTraineeInput } from "../core/shared/parseTraineeInput";
import {
  readDaemonView,
  recoverySummary,
  reportSummary,
  sessionLocked,
} from "./daemonView";

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing popup element: #${id}`);
  return found as T;
}
const ui = {
  token: element<HTMLInputElement>("daemon-token"),
  port: element<HTMLInputElement>("daemon-port"),
  save: element<HTMLButtonElement>("save-daemon"),
  connection: element<HTMLParagraphElement>("connection"),
  help: element<HTMLElement>("daemon-help"),
  origin: element<HTMLElement>("extension-origin"),
  command: element<HTMLElement>("daemon-command"),
  loadOptions: element<HTMLButtonElement>("load-options"),
  state: element<HTMLSpanElement>("state"),
  instructor: element<HTMLSelectElement>("instructor"),
  trainingType: element<HTMLSelectElement>("training-type"),
  trainingDate: element<HTMLInputElement>("training-date"),
  trainees: element<HTMLTextAreaElement>("trainees"),
  current: element<HTMLParagraphElement>("current"),
  start: element<HTMLButtonElement>("start"),
  status: element<HTMLParagraphElement>("status"),
  recoverySection: element<HTMLElement>("recovery-section"),
  recovery: element<HTMLParagraphElement>("recovery"),
  reportSection: element<HTMLElement>("report-section"),
  report: element<HTMLParagraphElement>("report"),
};
const counters = [
  "total",
  "processed",
  "successful",
  "failed",
  "skipped",
  "indeterminate",
  "remaining",
] as const;
const origin = extensionOrigin(chrome.runtime.getURL(""));
ui.origin.textContent = origin;
ui.port.value = String(DEFAULT_DAEMON_PORT);

let client: DaemonClient | null = null;
let connected = false;
let optionsReady = false;
let polling = false;
let busy = false;
let viewGeneration = 0;

function status(text: string, error = false): void {
  ui.status.textContent = text;
  ui.status.dataset.tone = error ? "error" : "info";
}
function controls(): void {
  const locked =
    !connected || busy || sessionLocked(ui.state.dataset.state ?? "unknown");
  ui.start.disabled = locked || !optionsReady;
  ui.loadOptions.disabled = locked;
  ui.instructor.disabled = locked || !optionsReady;
  ui.trainingType.disabled = locked || !optionsReady;
  ui.trainingDate.disabled = locked;
  ui.trainees.disabled = locked;
  ui.save.disabled = busy;
}
function disconnected(reason: string): void {
  connected = false;
  ui.connection.textContent = reason;
  ui.help.hidden = false;
  ui.state.textContent = "disconnected";
  delete ui.state.dataset.state;
  ui.current.textContent = "Daemon state unavailable (execution may continue).";
  for (const key of counters) element(key).textContent = "—";
  ui.reportSection.hidden = true;
  ui.recoverySection.hidden = true;
  controls();
}
function command(port: number): void {
  // The command contains only validated identity/port, never the secret token.
  ui.command.textContent = startupCommand(origin, port);
}
function selectOptions(
  select: HTMLSelectElement,
  values: TrainingFormOption[],
  placeholder: string,
): void {
  select.replaceChildren();
  const empty = document.createElement("option");
  empty.value = "";
  empty.textContent = placeholder;
  select.append(empty);
  for (const value of values) {
    const option = document.createElement("option");
    option.value = value.value;
    option.textContent = value.label;
    select.append(option);
  }
}
function resetOptions(): void {
  optionsReady = false;
  selectOptions(ui.instructor, [], "Load instructors explicitly");
  selectOptions(ui.trainingType, [], "Load training types explicitly");
}
async function refresh(): Promise<void> {
  if (!client || polling) return;
  polling = true;
  const requestedClient = client;
  const requestedGeneration = viewGeneration;
  try {
    const view = await readDaemonView(requestedClient);
    if (client !== requestedClient || viewGeneration !== requestedGeneration)
      return;
    connected = true;
    ui.connection.textContent =
      "Daemon connected. Rust owns execution and recovery.";
    ui.help.hidden = true;
    ui.state.textContent = view.progress.state.replace(/_/g, " ");
    ui.state.dataset.state = view.progress.state;
    for (const key of counters)
      element(key).textContent = String(view.progress[key]);
    ui.current.textContent =
      view.progress.currentTraineeName ?? "No current trainee reported";
    ui.reportSection.hidden = view.report === null;
    ui.report.textContent = view.report ? reportSummary(view.report) : "";
    const recovery = recoverySummary(view.checkpoint);
    ui.recoverySection.hidden = recovery === null;
    ui.recovery.textContent = recovery ?? "";
    controls();
  } catch (error) {
    if (client === requestedClient && viewGeneration === requestedGeneration)
      disconnected(
        error instanceof Error ? error.message : "Daemon unavailable.",
      );
  } finally {
    polling = false;
  }
}

ui.save.addEventListener("click", () => {
  void (async () => {
    try {
      const settings = validateSettings({
        port: Number(ui.port.value),
        token: ui.token.value.trim(),
      });
      // Store connection settings only, never reports, queues, checkpoints or progress.
      await chrome.storage.local.set({ [DAEMON_SETTINGS_KEY]: settings });
      client = new DaemonClient(settings);
      command(settings.port);
      resetOptions();
      disconnected("Connecting to daemon…");
      status("Daemon settings saved. Token stays in extension-local storage.");
      await refresh();
    } catch (error) {
      status(
        error instanceof Error
          ? error.message
          : "Could not save daemon settings.",
        true,
      );
    }
  })();
});

ui.loadOptions.addEventListener("click", () => {
  if (!client || busy) return;
  const requestedClient = client;
  busy = true;
  controls();
  status(
    "Loading form options via Rust. This may open the worker and require manual portal login.",
  );
  void requestedClient
    .formOptions()
    .then((options) => {
      if (client !== requestedClient) return;
      selectOptions(ui.instructor, options.instructors, "Choose instructor");
      selectOptions(
        ui.trainingType,
        options.trainingTypes,
        "Choose training type",
      );
      optionsReady =
        options.instructors.length > 0 && options.trainingTypes.length > 0;
      status(
        optionsReady
          ? "Form options loaded."
          : "Daemon returned empty form options.",
        !optionsReady,
      );
    })
    .catch((error: unknown) => {
      resetOptions();
      status(
        error instanceof Error ? error.message : "Form options failed.",
        true,
      );
    })
    .finally(() => {
      busy = false;
      controls();
      void refresh();
    });
});

ui.start.addEventListener("click", () => {
  if (!client || busy || ui.start.disabled) return;
  const names = parseTraineeInput(ui.trainees.value);
  if (
    !names.length ||
    !ui.trainingDate.value ||
    !ui.instructor.value ||
    !ui.trainingType.value
  ) {
    status(
      "Trainee names, instructor, training type and date are required.",
      true,
    );
    return;
  }
  busy = true;
  controls();
  status("Requesting admission. A 202 is not submission confirmation.");
  void client
    .start({
      trainees: names.map((name) => ({ name })),
      session: {
        trainingDate: ui.trainingDate.value,
        instructor: ui.instructor.value,
        trainingType: ui.trainingType.value,
      },
      resume: false,
    })
    .then(({ batchId }) => {
      status(
        `Batch admitted (${batchId}). Polling Rust for progress and report.`,
      );
    })
    .catch((error: unknown) => {
      // In particular, a 409 is shown once: no resume acknowledgement or retry.
      status(
        error instanceof Error ? error.message : "Batch admission failed.",
        true,
      );
    })
    .finally(() => {
      // A read begun before admission completed cannot unlock a second start.
      viewGeneration += 1;
      connected = false;
      busy = false;
      controls();
      void refresh();
    });
});

command(DEFAULT_DAEMON_PORT);
resetOptions();
disconnected("Daemon not configured. Paste its token and save to connect.");
void (async () => {
  try {
    const stored = await chrome.storage.local.get(DAEMON_SETTINGS_KEY);
    const settings = stored[DAEMON_SETTINGS_KEY] as DaemonSettings | undefined;
    if (!settings) return;
    client = new DaemonClient(settings);
    ui.port.value = String(settings.port);
    ui.token.value = settings.token;
    command(settings.port);
    await refresh();
  } catch (error) {
    disconnected(
      error instanceof Error
        ? error.message
        : "Daemon configuration unavailable.",
    );
  }
})();
const timer = window.setInterval(() => {
  void refresh();
}, 1000);
window.addEventListener("unload", () => {
  window.clearInterval(timer);
});
