import type { PortalAdapter, SubmissionOutcome } from "./PortalAdapter";
import type { PortalCommand } from "./PortalCommands";
import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { TrainingFormOptions } from "../../domain/TrainingFormOptions";
import type { BrowserTabs } from "../browser/BrowserAdapter";
import {
  AutomationError,
  PortalUnavailableError,
  toAutomationError,
  type ErrorCode,
} from "../../shared/errors";

export class RemotePortalAdapter implements PortalAdapter {
  private static readonly COMMAND_TIMEOUT_MS = 15_000;
  private readonly tabs: BrowserTabs;

  /**
   * The tab this batch is driving, fixed at attach time.
   *
   * Resolving the active tab per command would let the target move mid-batch:
   * a batch runs for minutes while the operator is at their desk, and any tab
   * switch or newly focused window would redirect the next command. With a
   * second portal tab open that means filling one page and submitting on
   * another — a real training record written against the wrong trainee.
   */
  private tabId: number | null = null;

  constructor(tabs: BrowserTabs) {
    this.tabs = tabs;
  }

  async isPortalPage(): Promise<boolean> {
    const result = await this.dispatch<boolean>({ type: "PORTAL_IS_PAGE" });

    return result.success && result.data === true;
  }

  initializeTrainingSession(): Promise<Result<void>> {
    return this.dispatch<void>({ type: "PORTAL_INITIALIZE_SESSION" });
  }

  /**
   * Collapses a dispatch failure to `false`, because the caller's question is
   * "can I submit against this session?" and an unreachable content script
   * answers no. The engine's response either way is to reinitialise, which
   * surfaces the underlying error properly.
   */
  async isTrainingSessionReady(): Promise<boolean> {
    const result = await this.dispatch<boolean>({
      type: "PORTAL_SESSION_READY",
    });

    return result.success && result.data === true;
  }

  /**
   * Bind to the currently active tab for the duration of a batch.
   *
   * Call once before the run and `detach()` when it ends. Commands issued while
   * unattached fail rather than guessing at a target.
   */
  async attach(): Promise<Result<number>> {
    const tab = await this.tabs.findDsspTraineeTab();
    console.debug("[DSSP-DEBUG][BACKGROUND] DSSP trainee tab", {
      ...tab,
      consideredDsspTab: Boolean(
        tab?.url?.startsWith("https://dssp.frsc.gov.ng/Trainee"),
      ),
    });
    const tabId = tab?.id;

    if (tabId === undefined) {
      this.tabId = null;

      return {
        success: false,
        error: new PortalUnavailableError(
          "No active tab is available for the portal.",
        ),
      };
    }

    this.tabId = tabId;

    return { success: true, data: tabId };
  }

  detach(): void {
    this.tabId = null;
  }

  /** The pinned tab, or null when unattached. Exposed for diagnostics. */
  get attachedTabId(): number | null {
    return this.tabId;
  }

  getTrainees(): Promise<Result<Trainee[]>> {
    return this.dispatch<Trainee[]>({
      type: "PORTAL_GET_TRAINEES",
    });
  }

  getFormOptions(): Promise<Result<TrainingFormOptions>> {
    return this.dispatch<TrainingFormOptions>({
      type: "PORTAL_GET_FORM_OPTIONS",
    });
  }

  openTraineeLogs(): Promise<Result<void>> {
    return this.dispatch<void>({ type: "PORTAL_OPEN_TRAINEE_LOGS" });
  }

  openTrainee(trainee: Trainee): Promise<Result<void>> {
    return this.dispatch<void>({
      type: "PORTAL_OPEN_TRAINEE",
      trainee,
    });
  }

  prepareTrainee(trainee: Trainee): Promise<Result<void>> {
    return this.dispatch<void>({
      type: "PORTAL_PREPARE_TRAINEE",
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
    const tabId = this.tabId;

    if (tabId === null) {
      return {
        success: false,
        error: new PortalUnavailableError(
          "Portal adapter is not attached to a tab.",
        ),
      };
    }

    try {
      console.debug("[DSSP-DEBUG][BACKGROUND] sending message", {
        tabId,
        action: command.type,
      });
      const response = await Promise.race([
        this.tabs.sendMessage(tabId, command),
        new Promise<never>((_, reject) =>
          setTimeout(
            () => reject(new Error(`Timed out waiting for content script response (${command.type})`)),
            RemotePortalAdapter.COMMAND_TIMEOUT_MS,
          ),
        ),
      ]);
      console.debug("[DSSP-DEBUG][BACKGROUND] message response", {
        tabId,
        action: command.type,
        response,
      });

      return this.parse<T>(response);
    } catch (error) {
      console.error("[DSSP-DEBUG][BACKGROUND] message failed", {
        tabId,
        action: command.type,
        error,
      });
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
