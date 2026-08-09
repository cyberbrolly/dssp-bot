import { MessageBus } from "../core/infrastructure/messaging/MessageBus";
import { ChromiumBrowserAdapter } from "../core/infrastructure/browser/ChromiumBrowserAdapter";
import { isActive } from "../core/automation/AutomationState";
import type {
  BatchProgress,
  Message,
} from "../core/infrastructure/messaging/Messages";
import type { BatchReport } from "../core/domain/BatchReport";
import type { AutomationState } from "../core/automation/AutomationState";

const browser = new ChromiumBrowserAdapter();
const messageBus = new MessageBus(browser.runtime);

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);

  if (!found) {
    throw new Error(`Missing popup element: #${id}`);
  }

  return found as T;
}

const ui = {
  state: element<HTMLSpanElement>("state"),
  instructor: element<HTMLInputElement>("instructor"),
  trainingType: element<HTMLInputElement>("training-type"),
  trainingDate: element<HTMLInputElement>("training-date"),
  trainees: element<HTMLTextAreaElement>("trainees"),
  current: element<HTMLParagraphElement>("current"),
  total: element<HTMLSpanElement>("total"),
  processed: element<HTMLSpanElement>("processed"),
  successful: element<HTMLSpanElement>("successful"),
  failed: element<HTMLSpanElement>("failed"),
  skipped: element<HTMLSpanElement>("skipped"),
  indeterminate: element<HTMLSpanElement>("indeterminate"),
  remaining: element<HTMLSpanElement>("remaining"),
  start: element<HTMLButtonElement>("start"),
  pause: element<HTMLButtonElement>("pause"),
  resume: element<HTMLButtonElement>("resume"),
  stop: element<HTMLButtonElement>("stop"),
  status: element<HTMLParagraphElement>("status"),
  reportSection: element<HTMLElement>("report-section"),
  report: element<HTMLParagraphElement>("report"),
};

function setStatus(text: string, tone: "info" | "error" = "info"): void {
  ui.status.textContent = text;
  ui.status.dataset.tone = tone;
}

function parseTraineeIds(): string[] {
  return ui.trainees.value
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function renderControls(state: AutomationState): void {
  const running = isActive(state);

  ui.start.disabled = running || state === "paused";
  ui.pause.disabled = !running;
  ui.resume.disabled = state !== "paused";
  ui.stop.disabled = !running && state !== "paused";
}

function renderProgress(progress: BatchProgress): void {
  ui.state.textContent = progress.state.replace(/_/g, " ");
  ui.total.textContent = String(progress.total);
  ui.processed.textContent = String(progress.processed);
  ui.successful.textContent = String(progress.successful);
  ui.failed.textContent = String(progress.failed);
  ui.skipped.textContent = String(progress.skipped);
  ui.indeterminate.textContent = String(progress.indeterminate);
  ui.remaining.textContent = String(progress.remaining);
  ui.current.textContent = progress.currentTraineeName ?? "No batch running";

  renderControls(progress.state);
}

function renderReport(report: BatchReport | null): void {
  if (!report) {
    ui.reportSection.hidden = true;

    return;
  }

  ui.reportSection.hidden = false;

  const rate = Math.round(report.successRate * 100);

  const summary = `${report.successful}/${report.total} recorded, ${report.failed} failed, ${report.skipped} skipped, ${rate}% success rate.`;

  ui.report.textContent =
    report.indeterminate > 0
      ? `${summary} ${report.indeterminate} submitted without confirmation — check these on the portal before re-running them.`
      : summary;
}

async function send(message: Message): Promise<unknown> {
  try {
    const response = await messageBus.send(message);

    if (!response.success) {
      setStatus(response.error ?? "The request failed.", "error");

      return null;
    }

    return response.data ?? null;
  } catch (error) {
    setStatus(
      error instanceof Error
        ? error.message
        : "The background worker did not respond.",
      "error",
    );

    return null;
  }
}

async function refresh(): Promise<void> {
  const progress = await send({ type: "GET_STATUS" });

  if (progress) {
    renderProgress(progress as BatchProgress);
  }

  const report = await send({ type: "GET_REPORT" });

  renderReport((report as BatchReport | null) ?? null);
}

function onClick(
  button: HTMLButtonElement,
  handler: () => Promise<void>,
): void {
  button.addEventListener("click", () => {
    void handler();
  });
}

onClick(ui.start, async () => {
  const traineeIds = parseTraineeIds();

  if (traineeIds.length === 0) {
    setStatus("Add at least one trainee ID.", "error");

    return;
  }

  if (
    !ui.instructor.value.trim() ||
    !ui.trainingType.value.trim() ||
    !ui.trainingDate.value
  ) {
    setStatus("Instructor, training type, and date are required.", "error");

    return;
  }

  setStatus(`Starting ${traineeIds.length} trainee(s).`);

  const report = await send({
    type: "START_AUTOMATION",
    traineeIds,
    session: {
      instructorId: ui.instructor.value.trim(),
      trainingTypeId: ui.trainingType.value.trim(),
      trainingDate: ui.trainingDate.value,
    },
  });

  if (report) {
    renderReport(report as BatchReport);
    setStatus("Batch finished.");
  }

  await refresh();
});

onClick(ui.pause, async () => {
  await send({ type: "PAUSE_AUTOMATION" });
  setStatus("Paused after the current trainee.");
  await refresh();
});

onClick(ui.resume, async () => {
  await send({ type: "RESUME_AUTOMATION" });
  setStatus("Resumed.");
  await refresh();
});

onClick(ui.stop, async () => {
  await send({ type: "STOP_AUTOMATION" });
  setStatus("Stopping. Remaining trainees are skipped.");
  await refresh();
});

const pollTimer = window.setInterval(() => {
  void refresh();
}, 1000);

window.addEventListener("unload", () => {
  window.clearInterval(pollTimer);
});

void refresh();
