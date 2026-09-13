import { TaskQueue } from "./TaskQueue";
import { StateMachine, type AutomationState } from "./AutomationState";
import { TrainingLogger } from "./TrainingLogger";
import { RetryPolicy } from "../shared/retry";
import { delay } from "../shared/delay";
import {
  ConfirmationUnknownError,
  DuplicateRecordError,
  SessionExpiredError,
  SubmissionError,
  toAutomationError,
  type AutomationError,
  type ErrorCode,
} from "../shared/errors";
import type { Result } from "../shared/Result";
import type { Trainee } from "../domain/Trainee";
import type { TrainingSession } from "../domain/TrainingSession";
import type { TrainingOutcome, TrainingResult } from "../domain/TrainingResult";
import { buildBatchReport, type BatchReport } from "../domain/BatchReport";
import type {
  BatchCheckpoint,
  BatchCheckpointStatus,
  CheckpointWriter,
} from "./BatchCheckpoint";
import type {
  PortalAdapter,
  SubmissionOutcome,
} from "../infrastructure/portal/PortalAdapter";
import type { BatchProgress } from "../infrastructure/messaging/Messages";

export interface AutomationEngineOptions {
  portal: PortalAdapter;
  logger?: TrainingLogger;
  retryPolicy?: RetryPolicy;
  interTaskDelayMs?: number;
  /**
   * Persists progress after every settled trainee. Optional: without it the
   * engine behaves exactly as before, which is what the unit tests want, but
   * the service worker must supply one or a batch lost to worker termination
   * leaves no record of what it already submitted.
   */
  checkpoint?: CheckpointWriter;
}

export interface BatchTask {
  trainee: Trainee;
  session: Omit<TrainingSession, "traineeId">;
}

export class AutomationEngine {
  private readonly portal: PortalAdapter;
  private readonly logger: TrainingLogger;
  private readonly retryPolicy: RetryPolicy;
  private readonly interTaskDelayMs: number;
  private readonly machine = new StateMachine();
  private readonly queue = new TaskQueue<BatchTask>();
  private readonly results: TrainingResult[] = [];

  private stopRequested = false;
  private pauseRequested = false;
  private resumeSignal: (() => void) | null = null;
  private running = false;
  private currentTrainee: Trainee | null = null;
  private batchStartedAt = "";
  private report: BatchReport | null = null;
  private totalQueued = 0;
  private readonly checkpoint: CheckpointWriter | undefined;

  constructor(options: AutomationEngineOptions) {
    this.portal = options.portal;
    this.logger = options.logger ?? new TrainingLogger();
    this.retryPolicy = options.retryPolicy ?? new RetryPolicy();
    this.interTaskDelayMs = options.interTaskDelayMs ?? 750;
    this.checkpoint = options.checkpoint;
  }

  /**
   * Write the current position to durable storage.
   *
   * Failures are swallowed on purpose. A storage error must not abort a batch
   * that is otherwise succeeding — aborting would strand a trainee mid-flow,
   * which is worse than a missing checkpoint. Reporting is the writer's job;
   * it has the logger and knows why its own write failed.
   */
  private async saveCheckpoint(status: BatchCheckpointStatus): Promise<void> {
    if (!this.checkpoint) {
      return;
    }

    const snapshot: BatchCheckpoint = {
      status,
      startedAt: this.batchStartedAt,
      updatedAt: new Date().toISOString(),
      total: this.totalQueued,
      results: [...this.results],
      pending: this.queue.ids(),
    };

    try {
      await this.checkpoint(snapshot);
    } catch {
      // Intentionally ignored — see above.
    }
  }

  getState(): AutomationState {
    return this.machine.state;
  }

  getQueueSize(): number {
    return this.queue.size;
  }

  getReport(): BatchReport | null {
    return this.report;
  }

  getLogEntries(): ReturnType<TrainingLogger["getEntries"]> {
    return this.logger.getEntries();
  }

  getProgress(): BatchProgress {
    return {
      state: this.machine.state,
      total: this.totalQueued,
      processed: this.results.length,
      successful: this.countOutcome("success"),
      failed: this.countOutcome("failed"),
      skipped: this.countOutcome("skipped"),
      indeterminate: this.countOutcome("indeterminate"),
      remaining: this.queue.size,
      ...(this.currentTrainee === null
        ? {}
        : { currentTraineeName: this.currentTrainee.name }),
    };
  }

  addTask(task: BatchTask): void {
    this.queue.enqueue({
      id: task.trainee.id,
      payload: task,
    });

    this.totalQueued += 1;
  }

  addTasks(tasks: BatchTask[]): void {
    for (const task of tasks) {
      this.addTask(task);
    }
  }

  clearQueue(): void {
    this.queue.clear();
    this.totalQueued = 0;
  }

  async run(): Promise<Result<BatchReport>> {
    if (this.running) {
      return {
        success: false,
        error: new Error("Automation is already running."),
      };
    }

    this.running = true;
    this.stopRequested = false;
    this.pauseRequested = false;
    this.results.length = 0;
    this.report = null;
    this.batchStartedAt = new Date().toISOString();
    // Authoritative for this batch, so a previous run's count cannot leak in.
    this.totalQueued = this.queue.size;

    this.machine.reset();
    this.machine.transitionTo("initializing");
    this.logger.record("batch_start", "started");

    try {
      while (!this.queue.isEmpty && !this.stopRequested) {
        await this.waitWhilePaused();

        if (this.stopRequested) {
          break;
        }

        const task = this.queue.dequeue();

        if (!task) {
          break;
        }

        const result = await this.processTask(task.payload);

        this.results.push(result);
        await this.saveCheckpoint("running");

        if (this.shouldAbortBatch(result)) {
          this.drainQueueAsSkipped(
            result.outcome === "indeterminate"
              ? `Batch aborted: ${result.traineeName} was submitted but could not be confirmed. Check that record on the portal before re-running.`
              : "Batch aborted: the portal session is no longer usable.",
          );
          break;
        }

        if (!this.queue.isEmpty && !this.stopRequested) {
          await delay(this.interTaskDelayMs);
        }
      }

      if (this.stopRequested) {
        this.drainQueueAsSkipped("Batch stopped by the administrator.");
      }

      return await this.finish(this.stopRequested ? "stopped" : "complete");
    } catch (error) {
      const automationError = toAutomationError(error);

      this.logger.record("batch_complete", "failed", {}, automationError);

      await this.finish("stopped");

      return { success: false, error: automationError };
    } finally {
      this.running = false;
      this.pauseRequested = false;
      this.releasePause();
    }
  }

  pause(): void {
    if (!this.running || this.pauseRequested) {
      return;
    }

    this.pauseRequested = true;
    this.logger.record("pause", "started");
  }

  resume(): void {
    if (!this.pauseRequested) {
      return;
    }

    this.pauseRequested = false;
    this.releasePause();
    this.logger.record("resume", "started");
  }

  stop(): void {
    if (!this.running) {
      this.machine.reset();

      return;
    }

    this.stopRequested = true;
    this.pauseRequested = false;
    this.releasePause();
    this.logger.record("stop", "started");
  }

  private async finish(
    finalState: "stopped" | "complete",
  ): Promise<Result<BatchReport>> {
    this.currentTrainee = null;

    if (this.machine.canTransitionTo(finalState)) {
      this.machine.transitionTo(finalState);
    }

    this.report = buildBatchReport(
      [...this.results],
      this.batchStartedAt,
      new Date().toISOString(),
    );

    // Terminal either way: a stopped or crashed batch is as final as a complete
    // one, and its results are the ones most worth keeping, since they say what
    // reached the portal before things went wrong.
    await this.saveCheckpoint("finished");

    if (finalState === "complete") {
      this.logger.record("batch_complete", "succeeded");
    }

    return { success: true, data: this.report };
  }

  private countOutcome(outcome: TrainingResult["outcome"]): number {
    return this.results.filter((result) => result.outcome === outcome).length;
  }

  private shouldAbortBatch(result: TrainingResult): boolean {
    // An unconfirmed submission stops the batch. If the outcome of one write
    // cannot be read back, the same is likely true of every write after it, and
    // continuing would submit real records that nobody can account for.
    return (
      result.outcome === "indeterminate" ||
      result.errorCode === "SESSION_EXPIRED" ||
      result.errorCode === "PORTAL_STRUCTURE_CHANGED"
    );
  }

  private drainQueueAsSkipped(reason: string): void {
    const now = new Date().toISOString();

    while (!this.queue.isEmpty) {
      const task = this.queue.dequeue();

      if (!task) {
        break;
      }

      const { trainee } = task.payload;

      this.logger.record("stop", "skipped", {
        traineeId: trainee.id,
        traineeName: trainee.name,
      });

      this.results.push({
        traineeId: trainee.id,
        traineeName: trainee.name,
        outcome: "skipped",
        attempts: 0,
        startedAt: now,
        finishedAt: now,
        errorMessage: reason,
      });
    }
  }

  private async waitWhilePaused(): Promise<void> {
    if (!this.pauseRequested) {
      return;
    }

    if (this.machine.canTransitionTo("paused")) {
      this.machine.transitionTo("paused");
    }

    // The riskiest moment in the batch. A paused engine makes no extension API
    // calls, so nothing holds the service worker open and it is collected after
    // roughly 30 seconds — taking the queue and every result with it. Flushing
    // here means a batch that never wakes up still leaves a record of what it
    // had already written to the portal.
    await this.saveCheckpoint("paused");

    return new Promise<void>((resolve) => {
      this.resumeSignal = resolve;
    });
  }

  private releasePause(): void {
    const signal = this.resumeSignal;

    this.resumeSignal = null;
    signal?.();
  }

  /**
   * Runs one trainee in three phases with deliberately different retry rules:
   *
   * 1. prepare — reads and form filling only. Nothing reaches the portal, so the
   *    whole phase is replayed on a recoverable failure.
   * 2. commit  — the single submitting call. Never replayed: a second attempt
   *    would create a second training record.
   * 3. confirm — reads the outcome back. Safe to retry, but if it never yields
   *    an answer the result is `indeterminate`, not a failure, because the
   *    record may already exist.
   */
  private async processTask(task: BatchTask): Promise<TrainingResult> {
    const { trainee, session } = task;
    const startedAt = new Date().toISOString();

    this.currentTrainee = trainee;

    const context = {
      traineeId: trainee.id,
      traineeName: trainee.name,
    };

    let attempts = 0;

    const settle = (
      outcome: TrainingOutcome,
      error?: AutomationError,
    ): TrainingResult => ({
      traineeId: trainee.id,
      traineeName: trainee.name,
      outcome,
      attempts,
      startedAt,
      finishedAt: new Date().toISOString(),
      ...(error === undefined
        ? {}
        : { errorCode: error.code, errorMessage: error.message }),
    });

    try {
      await this.retryPolicy.execute(async (attempt) => {
        attempts = attempt;

        if (attempt > 1) {
          this.transition("retrying");
          this.logger.record("retry", "started", { ...context, attempt });
        }

        await this.prepareSubmission(
          trainee,
          { ...session, traineeId: trainee.id },
          attempt,
        );
      });
    } catch (error) {
      const automationError = toAutomationError(error);

      this.logger.record(
        "fill_form",
        "failed",
        { ...context, attempt: attempts },
        automationError,
      );

      return settle("failed", automationError);
    }

    const committed = await this.commitSubmission(context, attempts);

    if (!committed.success) {
      const automationError = toAutomationError(committed.error);

      // Only a failure that proves the request never left the browser can be
      // reported as a clean failure. Anything else may have reached the portal.
      return settle(
        provesNothingSubmitted(automationError.code)
          ? "failed"
          : "indeterminate",
        automationError,
      );
    }

    let outcome: SubmissionOutcome;

    try {
      outcome = await this.retryPolicy.execute((attempt) =>
        this.confirmSubmission({ ...context, attempt }),
      );
    } catch (error) {
      const automationError = new ConfirmationUnknownError(
        toAutomationError(error).message,
      );

      this.logger.record(
        "verify_result",
        "failed",
        { ...context, attempt: attempts },
        automationError,
      );

      return settle("indeterminate", automationError);
    }

    // The portal gave a definite answer, so the record's fate is known.
    if (outcome.status !== "confirmed") {
      const automationError =
        outcome.status === "duplicate"
          ? new DuplicateRecordError(outcome.message)
          : new SubmissionError(outcome.message);

      this.logger.record(
        "verify_result",
        "failed",
        { ...context, attempt: attempts },
        automationError,
      );

      return settle("failed", automationError);
    }

    this.logger.record("verify_result", "succeeded", {
      ...context,
      attempt: attempts,
    });

    return settle("success");
  }

  /** Phase 1. Safe to replay: no step here writes to the portal. */
  private async prepareSubmission(
    trainee: Trainee,
    session: TrainingSession,
    attempt: number,
  ): Promise<void> {
    const context = {
      traineeId: trainee.id,
      traineeName: trainee.name,
      attempt,
    };

    // Re-checked for every trainee and on every retry, not once per batch. The
    // session can lapse mid-run, and the check is worthless if it cannot catch
    // that — a lapsed session is also why a retry would otherwise keep failing.
    if (!(await this.portal.isPortalPage())) {
      throw new SessionExpiredError();
    }

    this.transition("loading_trainee");
    this.logger.record("load_trainee", "started", context);
    unwrap(await this.portal.openTrainee(trainee));
    this.logger.record("load_trainee", "succeeded", context);

    this.transition("opening_form");
    this.logger.record("open_form", "started", context);
    unwrap(await this.portal.openTrainingForm());
    this.logger.record("open_form", "succeeded", context);

    this.transition("filling_form");
    this.logger.record("fill_form", "started", context);
    unwrap(await this.portal.fillTrainingForm(session));
    this.logger.record("fill_form", "succeeded", context);

    this.transition("validating");
    this.logger.record("validate_form", "started", context);
    unwrap(await this.portal.validateTrainingForm());
    this.logger.record("validate_form", "succeeded", context);
  }

  /**
   * Phase 2. Called exactly once per trainee and never through the retry
   * policy. Returns the raw Result so the caller can decide, from the error
   * code alone, whether the submission might have landed.
   */
  private async commitSubmission(
    context: { traineeId: string; traineeName: string },
    attempt: number,
  ): Promise<Result<void>> {
    this.transition("submitting");
    this.logger.record("submit_form", "started", { ...context, attempt });

    const result = await this.portal.submitTrainingForm();

    if (result.success) {
      this.logger.record("submit_form", "succeeded", { ...context, attempt });
    }

    return result;
  }

  /** Phase 3. Read-only, so retrying cannot submit a second record. */
  private async confirmSubmission(context: {
    traineeId: string;
    traineeName: string;
    attempt: number;
  }): Promise<SubmissionOutcome> {
    this.transition("verifying");
    this.logger.record("verify_result", "started", context);

    return unwrap(await this.portal.waitForSubmissionResult());
  }

  private transition(next: AutomationState): void {
    if (this.machine.canTransitionTo(next)) {
      this.machine.transitionTo(next);
    }
  }
}

function unwrap<T>(result: Result<T>): T {
  if (!result.success) {
    throw toAutomationError(result.error);
  }

  return result.data;
}

/**
 * Whether a submit failure proves nothing was sent. These codes are raised
 * before the request leaves the page — a missing button, an unmapped portal, a
 * dead session. Transport codes such as NETWORK and TIMEOUT are excluded on
 * purpose: the portal may have recorded the training anyway.
 */
function provesNothingSubmitted(code: ErrorCode): boolean {
  const proven: readonly ErrorCode[] = [
    "ELEMENT_NOT_FOUND",
    "PORTAL_NOT_MAPPED",
    "PORTAL_STRUCTURE_CHANGED",
    "SESSION_EXPIRED",
    "VALIDATION_FAILED",
    "MISSING_DATA",
    "TRAINEE_NOT_FOUND",
  ];

  return proven.includes(code);
}
