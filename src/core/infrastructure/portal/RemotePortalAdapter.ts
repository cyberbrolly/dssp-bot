import type { PortalAdapter, SubmissionOutcome } from "./PortalAdapter";
import type { PortalCommand } from "./PortalCommands";
import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { BrowserTabs } from "../browser/BrowserAdapter";
import {
  AutomationError,
  PortalUnavailableError,
  toAutomationError,
  type ErrorCode,
} from "../../shared/errors";

export class RemotePortalAdapter implements PortalAdapter {
  private readonly tabs: BrowserTabs;
  private portalPage = true;

  constructor(tabs: BrowserTabs) {
    this.tabs = tabs;
  }

  isPortalPage(): boolean {
    return this.portalPage;
  }

  async refreshPortalPage(): Promise<boolean> {
    const result = await this.dispatch<boolean>({
      type: "PORTAL_IS_PAGE",
    });

    this.portalPage = result.success && result.data;

    return this.portalPage;
  }

  getTrainees(): Promise<Result<Trainee[]>> {
    return this.dispatch<Trainee[]>({
      type: "PORTAL_GET_TRAINEES",
    });
  }

  openTrainee(trainee: Trainee): Promise<Result<void>> {
    return this.dispatch<void>({
      type: "PORTAL_OPEN_TRAINEE",
      trainee,
    });
  }

  openTrainingForm(): Promise<Result<void>> {
    return this.dispatch<void>({
      type: "PORTAL_OPEN_FORM",
    });
  }

  fillTrainingForm(session: TrainingSession): Promise<Result<void>> {
    return this.dispatch<void>({
      type: "PORTAL_FILL_FORM",
      session,
    });
  }

  validateTrainingForm(): Promise<Result<void>> {
    return this.dispatch<void>({
      type: "PORTAL_VALIDATE_FORM",
    });
  }

  submitTrainingForm(): Promise<Result<void>> {
    return this.dispatch<void>({
      type: "PORTAL_SUBMIT_FORM",
    });
  }

  waitForSubmissionResult(): Promise<Result<SubmissionOutcome>> {
    return this.dispatch<SubmissionOutcome>({
      type: "PORTAL_WAIT_RESULT",
    });
  }

  private async dispatch<T>(command: PortalCommand): Promise<Result<T>> {
    try {
      const tabId = await this.tabs.getActiveTabId();

      if (tabId === undefined) {
        return {
          success: false,
          error: new PortalUnavailableError(
            "No active tab is available for the portal.",
          ),
        };
      }

      const response = await this.tabs.sendMessage(tabId, command);

      return this.parse<T>(response);
    } catch (error) {
      return {
        success: false,
        error: new PortalUnavailableError(toAutomationError(error).message),
      };
    }
  }

  private parse<T>(response: unknown): Result<T> {
    if (
      typeof response !== "object" ||
      response === null ||
      !("success" in response)
    ) {
      return {
        success: false,
        error: new PortalUnavailableError(
          "Malformed response received from the content script.",
        ),
      };
    }

    if (response.success === true) {
      return {
        success: true,
        data: ("data" in response ? response.data : undefined) as T,
      };
    }

    const message =
      "error" in response && typeof response.error === "string"
        ? response.error
        : "The portal operation failed.";

    const code =
      "code" in response && typeof response.code === "string"
        ? (response.code as ErrorCode)
        : "SUBMISSION_FAILED";

    return {
      success: false,
      error: new AutomationError(
        message,
        code,
        code === "ELEMENT_NOT_FOUND" ||
          code === "TIMEOUT" ||
          code === "NETWORK" ||
          code === "PORTAL_UNAVAILABLE",
      ),
    };
  }
}
