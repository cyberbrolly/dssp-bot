import { UnmappedPortalAdapter } from "../core/infrastructure/portal/UnmappedPortalAdapter";
import {
  isPortalCommand,
  type PortalCommand,
  type PortalCommandResponse,
} from "../core/infrastructure/portal/PortalCommands";
import { BridgeClient } from "../core/infrastructure/portal/BridgeClient";
import type { PortalAdapter } from "../core/infrastructure/portal/PortalAdapter";
import { Logger } from "../core/infrastructure/logging/Logger";
import { toAutomationError, type AutomationError } from "../core/shared/errors";
import type { Result } from "../core/shared/Result";

const logger = new Logger("DSSP:content");
const portal: PortalAdapter = new UnmappedPortalAdapter();

/**
 * Channel to the MAIN-world script, which owns the page's real `alert` and
 * `XMLHttpRequest`. Anything needing the page's own JS context goes through it.
 */
const bridge = new BridgeClient(window);

bridge.start();

// The portal signals success and failure through alerts and an XHR whose body
// is not yet characterised. Logging both is what makes that observable.
bridge.onEvent((event) => {
  if (event.type === "ALERT") {
    logger.info("Portal alert intercepted", { message: event.message });
    return;
  }

  logger.info("Portal submission response", {
    status: event.status,
    url: event.url,
    bodyLength: event.body.length,
    bodyPreview: event.body.slice(0, 200),
  });
});

function toResponse(result: Result<unknown>): PortalCommandResponse {
  if (result.success) {
    return { success: true, data: result.data };
  }

  const error = toAutomationError(result.error);

  return {
    success: false,
    error: error.message,
    code: error.code,
  };
}

function fromError(error: AutomationError): PortalCommandResponse {
  return {
    success: false,
    error: error.message,
    code: error.code,
  };
}

/**
 * How long dialog suppression stays armed around a single submitting command.
 *
 * Only an upper bound, not a wait: `withDialogsArmed` disarms as soon as the
 * command settles. It needs to outlast the slowest legitimate submit so the
 * deadline never fires mid-operation, while still being short enough that a
 * crashed content script leaves the page suppressed only briefly.
 */
const DIALOG_ARM_MS = 30_000;

/**
 * Commands that can provoke a portal dialog.
 *
 * The portal raises its alert from a jQuery submit handler, and the result read
 * follows immediately after, so both need suppression. Everything else — reads,
 * navigation, form filling — runs with the page's own dialogs intact, so an
 * administrator browsing alongside the extension still sees their own prompts.
 */
const SUPPRESSES_DIALOGS: ReadonlySet<PortalCommand["type"]> = new Set([
  "PORTAL_SUBMIT_FORM",
  "PORTAL_WAIT_RESULT",
]);

async function execute(command: PortalCommand): Promise<PortalCommandResponse> {
  if (SUPPRESSES_DIALOGS.has(command.type)) {
    return bridge.withDialogsArmed(DIALOG_ARM_MS, () => dispatch(command));
  }

  return dispatch(command);
}

async function dispatch(
  command: PortalCommand,
): Promise<PortalCommandResponse> {
  switch (command.type) {
    case "PORTAL_IS_PAGE":
      // This script only injects on hosts matching host_permissions, so its own
      // execution proves the origin. Requiring the MAIN half too means a broken
      // bridge is reported here rather than as a confusing failure mid-batch.
      //
      // Deliberately origin-level only: which page this is (trainee list vs
      // training log) is a separate question, still open pending page roles.
      return {
        success: true,
        data: await bridge.isReady(),
      };

    case "PORTAL_GET_TRAINEES":
      return toResponse(await portal.getTrainees());

    case "PORTAL_OPEN_TRAINEE":
      return toResponse(await portal.openTrainee(command.trainee));

    case "PORTAL_OPEN_FORM":
      return toResponse(await portal.openTrainingForm());

    case "PORTAL_FILL_FORM":
      return toResponse(await portal.fillTrainingForm(command.session));

    case "PORTAL_VALIDATE_FORM":
      return toResponse(await portal.validateTrainingForm());

    case "PORTAL_SUBMIT_FORM":
      return toResponse(await portal.submitTrainingForm());

    case "PORTAL_WAIT_RESULT":
      return toResponse(await portal.waitForSubmissionResult());

    default:
      return {
        success: false,
        error: `Unsupported portal command: ${
          (command as { type: string }).type
        }`,
      };
  }
}

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!isPortalCommand(message)) {
    return false;
  }

  execute(message)
    .then(sendResponse)
    .catch((error: unknown) => {
      const automationError = toAutomationError(error);

      logger.error("Portal command failed", {
        type: message.type,
        error: automationError.message,
      });

      sendResponse(fromError(automationError));
    });

  return true;
});
