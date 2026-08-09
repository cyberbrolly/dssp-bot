import { UnmappedPortalAdapter } from "../core/infrastructure/portal/UnmappedPortalAdapter";
import {
  isPortalCommand,
  type PortalCommand,
  type PortalCommandResponse,
} from "../core/infrastructure/portal/PortalCommands";
import type { PortalAdapter } from "../core/infrastructure/portal/PortalAdapter";
import { Logger } from "../core/infrastructure/logging/Logger";
import { toAutomationError, type AutomationError } from "../core/shared/errors";
import type { Result } from "../core/shared/Result";

const logger = new Logger("DSSP:content");
const portal: PortalAdapter = new UnmappedPortalAdapter();

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

async function execute(command: PortalCommand): Promise<PortalCommandResponse> {
  switch (command.type) {
    case "PORTAL_IS_PAGE":
      return {
        success: true,
        data: portal.isPortalPage(),
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
