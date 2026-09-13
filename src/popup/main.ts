import { MessageBus } from "../core/infrastructure/messaging/MessageBus";
import { ChromiumBrowserAdapter } from "../core/infrastructure/browser/ChromiumBrowserAdapter";
import { isActive } from "../core/automation/AutomationState";
import { unreconciled } from "../core/automation/BatchCheckpoint";
import type { BatchCheckpoint } from "../core/automation/BatchCheckpoint";
import type {
  BatchProgress,
  Message,
} from "../core/infrastructure/messaging/Messages";
import type { BatchReport } from "../core/domain/BatchReport";
import type { AutomationState } from "../core/automation/AutomationState";
import type {
  TrainingFormOption,
  TrainingFormOptions,
} from "../core/domain/TrainingFormOptions";
import { formatTrainingDate } from "../core/shared/trainingDate";

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
  instructor: element<HTMLSelectElement>("instructor"),
  trainingType: element<HTMLSelectElement>("training-type"),
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
  recoverySection: element<HTMLElement>("recovery-section"),
  recovery: element<HTMLParagraphElement>("recovery"),
  reportSection: element<HTMLElement>("report-section"),
  report: element<HTMLParagraphElement>("report"),
};

let formOptionsReady = false;
let cachedFormOptions: TrainingFormOptions | null = null;
let currentState: AutomationState = "idle";

function setStatus(text: string, tone: "info" | "error" = "info"): void {
  ui.status.textContent = text;
  ui.status.dataset.tone = tone;
}

function parseTraineeIds(): string[] {
  const raw = ui.trainees.value;
  const parsed = raw
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  console.debug("[DSSP:popup] Trainee IDs textarea read", {
    rawValue: raw,
    parsedValues: parsed,
    rawCodePoints: Array.from(raw, (char) => char.codePointAt(0)),
  });
  return parsed;
}

function renderControls(state: AutomationState): void {
  const running = isActive(state);
  const sessionLocked = running || state === "paused";

  currentState = state;

  ui.start.disabled = sessionLocked || !formOptionsReady;
  ui.pause.disabled = !running;
  ui.resume.disabled = state !== "paused";
  ui.stop.disabled = !running && state !== "paused";
  ui.instructor.disabled = sessionLocked || !formOptionsReady;
  ui.trainingType.disabled = sessionLocked || !formOptionsReady;
  ui.trainingDate.disabled = sessionLocked;
  ui.trainees.disabled = sessionLocked;
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

function isTrainingFormOption(value: unknown): value is TrainingFormOption {
  return (
    typeof value === "object" &&
    value !== null &&
    "value" in value &&
    typeof value.value === "string" &&
    "label" in value &&
    typeof value.label === "string"
  );
}

function isTrainingFormOptions(value: unknown): value is TrainingFormOptions {
  return (
    typeof value === "object" &&
    value !== null &&
    "instructors" in value &&
    Array.isArray(value.instructors) &&
    value.instructors.every(isTrainingFormOption) &&
    "trainingTypes" in value &&
    Array.isArray(value.trainingTypes) &&
    value.trainingTypes.every(isTrainingFormOption)
  );
}

function renderSelectOptions(
  select: HTMLSelectElement,
  options: TrainingFormOption[],
  placeholder: string,
): void {
  select.replaceChildren();

  const empty = document.createElement("option");
  empty.value = "";
  empty.textContent = placeholder;
  empty.disabled = true;
  empty.selected = true;
  select.append(empty);

  for (const option of options) {
    const element = document.createElement("option");
    element.value = option.value;
    element.textContent = option.label;
    select.append(element);
  }
}

async function loadFormOptions(): Promise<void> {
  if (cachedFormOptions) {
    renderSelectOptions(
      ui.instructor,
      cachedFormOptions.instructors,
      "Choose instructor",
    );
    renderSelectOptions(
      ui.trainingType,
      cachedFormOptions.trainingTypes,
      "Choose training type",
    );
    formOptionsReady = true;
    renderControls(currentState);
    return;
  }

  formOptionsReady = false;
  renderControls(currentState);
  setStatus("Loading form options.");

  const data = await send({ type: "GET_FORM_OPTIONS" });

  if (!isTrainingFormOptions(data)) {
    return;
  }

  cachedFormOptions = data;
  renderSelectOptions(ui.instructor, data.instructors, "Choose instructor");
  renderSelectOptions(
    ui.trainingType,
    data.trainingTypes,
    "Choose training type",
  );
  formOptionsReady = true;
  renderControls(currentState);
  setStatus("Form options loaded.");
}

/**
 * Warn about a batch that died with the service worker.
 *
 * Only `interrupted` is shown. A finished batch is already covered by the
 * report, and a live one is covered by the progress counters — but an
 * interrupted one is the only case where records may exist on the portal that
 * nothing in this UI would otherwise account for.
 */
function renderRecovery(checkpoint: BatchCheckpoint | null): void {
  if (!checkpoint || checkpoint.status !== "interrupted") {
    ui.recoverySection.hidden = true;

    return;
  }

  const { indeterminate, unprocessed } = unreconciled(checkpoint);

  const parts = [
    `A batch started ${checkpoint.startedAt.slice(0, 16).replace("T", " ")} did not finish.`,
    `${checkpoint.results.length} of ${checkpoint.total} trainees were processed.`,
  ];

  if (indeterminate.length > 0) {
    parts.push(
      `${indeterminate.length} were submitted without confirmation (${indeterminate
        .map((entry) => entry.traineeName)
        .join(", ")}) — check these on the portal before re-running them.`,
    );
  }

  if (unprocessed.length > 0) {
    parts.push(`${unprocessed.length} were never attempted.`);
  }

  ui.recovery.textContent = parts.join(" ");
  ui.recoverySection.hidden = false;
}

async function refresh(): Promise<void> {
  const progress = await send({ type: "GET_STATUS" });

  if (progress) {
    renderProgress(progress as BatchProgress);
  }

  const report = await send({ type: "GET_REPORT" });

  renderReport((report as BatchReport | null) ?? null);

  const checkpoint = await send({ type: "GET_CHECKPOINT" });

  renderRecovery((checkpoint as BatchCheckpoint | null) ?? null);
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
    !ui.instructor.value ||
    !ui.trainingType.value ||
    !ui.trainingDate.value
  ) {
    setStatus("Instructor, training type, and date are required.", "error");

    return;
  }

  const instructor = ui.instructor.selectedOptions[0];
  const trainingType = ui.trainingType.selectedOptions[0];

  if (!instructor || !trainingType) {
    setStatus("Choose an instructor and training type.", "error");

    return;
  }

  let trainingDate: string;

  try {
    trainingDate = formatTrainingDate(ui.trainingDate.value);
  } catch (error) {
    setStatus(
      error instanceof Error ? error.message : "Training date is invalid.",
      "error",
    );

    return;
  }

  setStatus(`Starting ${traineeIds.length} trainee(s).`);

  const report = await send({
    type: "START_AUTOMATION",
    traineeIds,
    session: {
      instructorId: ui.instructor.value,
      instructorLabel: instructor.textContent ?? "",
      trainingTypeId: ui.trainingType.value,
      trainingTypeLabel: trainingType.textContent ?? "",
      trainingDate,
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

void Promise.all([refresh(), loadFormOptions()]);
