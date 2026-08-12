import { AutomationEngine } from "../core/automation/AutomationEngine";
import { BatchRunner } from "../core/automation/BatchRunner";
import { MessageBus } from "../core/infrastructure/messaging/MessageBus";
import { ChromiumBrowserAdapter } from "../core/infrastructure/browser/ChromiumBrowserAdapter";
import { RemotePortalAdapter } from "../core/infrastructure/portal/RemotePortalAdapter";
import { Logger } from "../core/infrastructure/logging/Logger";
import { TrainingLogger } from "../core/automation/TrainingLogger";
import { Storage } from "../core/infrastructure/storage/Storage";
import {
  markInterrupted,
  unreconciled,
} from "../core/automation/BatchCheckpoint";
import type { BatchCheckpoint } from "../core/automation/BatchCheckpoint";
import type {
  Message,
  MessageResponse,
} from "../core/infrastructure/messaging/Messages";
import type { BatchReport } from "../core/domain/BatchReport";

const browser = new ChromiumBrowserAdapter();
const logger = new Logger("DSSP:background");
const storage = new Storage(browser.storage);
const portal = new RemotePortalAdapter(browser.tabs);
const trainingLogger = new TrainingLogger(logger);

const LAST_REPORT_KEY = "dssp.lastReport";
const CHECKPOINT_KEY = "dssp.checkpoint";

/**
 * Persist batch progress after every trainee.
 *
 * Owns its own error reporting: the engine deliberately ignores failures here
 * so a storage problem cannot abort a run mid-submission, which means this is
 * the only place a failed write becomes visible.
 */
async function writeCheckpoint(checkpoint: BatchCheckpoint): Promise<void> {
  try {
    await storage.set<BatchCheckpoint>(CHECKPOINT_KEY, checkpoint);
  } catch (error) {
    logger.error("Failed to persist batch checkpoint", {
      status: checkpoint.status,
      processed: checkpoint.results.length,
      reason: error instanceof Error ? error.message : String(error),
    });
  }
}

const engine = new AutomationEngine({
  portal,
  logger: trainingLogger,
  checkpoint: writeCheckpoint,
});

const runner = new BatchRunner(portal, engine);
const messageBus = new MessageBus(browser.runtime);

/**
 * Reconcile a checkpoint left behind by a previous worker.
 *
 * A checkpoint still marked `running` or `paused` means the last service worker
 * was terminated mid-batch — most likely while paused, since a paused engine
 * makes no API calls to hold the worker open. The trainees it had already
 * submitted are real records on the portal, so the checkpoint is re-marked
 * `interrupted` and kept for the operator rather than cleared.
 */
async function recoverInterruptedBatch(): Promise<void> {
  const stored = await storage.get<BatchCheckpoint>(CHECKPOINT_KEY);

  if (!stored) {
    return;
  }

  const reconciled = markInterrupted(stored);

  if (reconciled === stored) {
    return;
  }

  await storage.set<BatchCheckpoint>(CHECKPOINT_KEY, reconciled);

  const { indeterminate, unprocessed } = unreconciled(reconciled);

  logger.warn("Recovered an interrupted batch", {
    startedAt: reconciled.startedAt,
    processed: reconciled.results.length,
    total: reconciled.total,
    unconfirmed: indeterminate.length,
    neverAttempted: unprocessed.length,
  });
}

async function startBatch(
  message: Extract<Message, { type: "START_AUTOMATION" }>,
): Promise<MessageResponse> {
  // Bind to whichever tab is active now and hold it for the whole batch. The
  // operator will keep using their browser while this runs, so the target must
  // not follow focus.
  const attached = await portal.attach();

  if (!attached.success) {
    return {
      success: false,
      error: attached.error.message,
    };
  }

  try {
    // Pre-flight only, to fail a bad tab before any work starts. The engine
    // re-checks per trainee, so this is not the guard that matters mid-batch.
    if (!(await portal.isPortalPage())) {
      return {
        success: false,
        error: "The active tab is not a supported DSSP portal page.",
      };
    }

    const result = await runner.start({
      traineeIds: message.traineeIds,
      session: message.session,
    });

    // A failed batch has usually already written records to the portal, so its
    // partial report is persisted on the same footing as a successful one.
    // Returning early here previously discarded exactly the evidence needed to
    // work out what had been submitted.
    const report = result.success ? result.data : engine.getReport();

    if (report) {
      await storage.set<BatchReport>(LAST_REPORT_KEY, report);
    }

    if (!result.success) {
      return {
        success: false,
        error: result.error.message,
      };
    }

    return { success: true, data: result.data };
  } finally {
    // Release the pin however the batch ended, so a later run rebinds to the
    // tab the operator has chosen by then rather than inheriting a stale one.
    portal.detach();
  }
}

async function handle(message: Message): Promise<MessageResponse> {
  switch (message.type) {
    case "GET_STATUS":
      return {
        success: true,
        data: engine.getProgress(),
      };

    case "START_AUTOMATION":
      return startBatch(message);

    case "PAUSE_AUTOMATION":
      engine.pause();

      return {
        success: true,
        data: engine.getProgress(),
      };

    case "RESUME_AUTOMATION":
      engine.resume();

      return {
        success: true,
        data: engine.getProgress(),
      };

    case "STOP_AUTOMATION":
      engine.stop();

      return {
        success: true,
        data: engine.getProgress(),
      };

    case "GET_REPORT": {
      const report =
        engine.getReport() ??
        (await storage.get<BatchReport>(LAST_REPORT_KEY)) ??
        null;

      return { success: true, data: report };
    }

    case "GET_CHECKPOINT": {
      const checkpoint =
        (await storage.get<BatchCheckpoint>(CHECKPOINT_KEY)) ?? null;

      return { success: true, data: checkpoint };
    }

    default:
      return {
        success: false,
        error: `Unsupported message type: ${
          (message as { type: string }).type
        }`,
      };
  }
}

messageBus.listen(async (message) => {
  logger.debug("Received message", {
    type: message?.type,
  });

  return handle(message);
});

// Runs on every worker start, not just install. That is the point: an ordinary
// idle-collection restart is exactly how a batch gets interrupted.
void recoverInterruptedBatch().catch((error: unknown) => {
  logger.error("Checkpoint recovery failed", {
    reason: error instanceof Error ? error.message : String(error),
  });
});
