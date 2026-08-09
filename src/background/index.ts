import { AutomationEngine } from "../core/automation/AutomationEngine";
import { BatchRunner } from "../core/automation/BatchRunner";
import { MessageBus } from "../core/infrastructure/messaging/MessageBus";
import { ChromiumBrowserAdapter } from "../core/infrastructure/browser/ChromiumBrowserAdapter";
import { RemotePortalAdapter } from "../core/infrastructure/portal/RemotePortalAdapter";
import { Logger } from "../core/infrastructure/logging/Logger";
import { TrainingLogger } from "../core/automation/TrainingLogger";
import { Storage } from "../core/infrastructure/storage/Storage";
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

const engine = new AutomationEngine({
  portal,
  logger: trainingLogger,
});

const runner = new BatchRunner(portal, engine);
const messageBus = new MessageBus(browser.runtime);

const LAST_REPORT_KEY = "dssp.lastReport";

async function startBatch(
  message: Extract<Message, { type: "START_AUTOMATION" }>,
): Promise<MessageResponse> {
  if (!(await portal.refreshPortalPage())) {
    return {
      success: false,
      error: "The active tab is not a supported DSSP portal page.",
    };
  }

  const result = await runner.start({
    traineeIds: message.traineeIds,
    session: message.session,
  });

  if (!result.success) {
    return {
      success: false,
      error: result.error.message,
    };
  }

  await storage.set<BatchReport>(LAST_REPORT_KEY, result.data);

  return { success: true, data: result.data };
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
