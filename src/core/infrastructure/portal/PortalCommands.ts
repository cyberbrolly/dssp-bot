import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { TrainingFormOptions } from "../../domain/TrainingFormOptions";

/**
 * The wire form of a `PortalAdapter` call, sent from the background worker to
 * the content script.
 *
 * Every `PortalAdapter` method needs an arm here and a case in
 * `content-script.ts`, or the call cannot cross the boundary at all. The session
 * operations were once missing from both, so `RemotePortalAdapter` had no way to
 * forward them even in principle. `PortalAdapterWiring.test.ts` now asserts the
 * three-way correspondence.
 */
export type PortalCommand =
  | { type: "PORTAL_IS_PAGE" }
  | { type: "PORTAL_INITIALIZE_SESSION" }
  | { type: "PORTAL_SESSION_READY" }
  | { type: "PORTAL_GET_TRAINEES" }
  | { type: "PORTAL_GET_FORM_OPTIONS" }
  | { type: "PORTAL_OPEN_TRAINEE_LOGS" }
  | { type: "PORTAL_OPEN_TRAINEE"; trainee: Trainee }
  | { type: "PORTAL_PREPARE_TRAINEE"; trainee: Trainee }
  | { type: "PORTAL_OPEN_FORM" }
  | { type: "PORTAL_FILL_FORM"; session: TrainingSession }
  | { type: "PORTAL_VALIDATE_FORM" }
  | { type: "PORTAL_SUBMIT_FORM" }
  | { type: "PORTAL_WAIT_RESULT" };

export type PortalCommandType = PortalCommand["type"];

/**
 * Which `PortalAdapter` method each command carries.
 *
 * Exported so a test can assert that the adapter interface, this union, and the
 * content-script dispatcher all describe the same set of operations. Keyed by
 * `PortalCommandType`, so adding an arm without mapping it fails to compile.
 */
export const PORTAL_COMMAND_METHODS: Record<PortalCommandType, string> = {
  PORTAL_IS_PAGE: "isPortalPage",
  PORTAL_INITIALIZE_SESSION: "initializeTrainingSession",
  PORTAL_SESSION_READY: "isTrainingSessionReady",
  PORTAL_GET_TRAINEES: "getTrainees",
  PORTAL_GET_FORM_OPTIONS: "getFormOptions",
  PORTAL_OPEN_TRAINEE_LOGS: "openTraineeLogs",
  PORTAL_OPEN_TRAINEE: "openTrainee",
  PORTAL_PREPARE_TRAINEE: "prepareTrainee",
  PORTAL_OPEN_FORM: "openTrainingForm",
  PORTAL_FILL_FORM: "fillTrainingForm",
  PORTAL_VALIDATE_FORM: "validateTrainingForm",
  PORTAL_SUBMIT_FORM: "submitTrainingForm",
  PORTAL_WAIT_RESULT: "waitForSubmissionResult",
};

export type PortalCommandResponse =
  | { success: true; data: unknown }
  | { success: false; error: string; code?: string };

export function isPortalCommand(message: unknown): message is PortalCommand {
  return (
    typeof message === "object" &&
    message !== null &&
    "type" in message &&
    typeof message.type === "string" &&
    message.type.startsWith("PORTAL_")
  );
}

export type PortalResultOf<T extends PortalCommandType> =
  T extends "PORTAL_GET_TRAINEES"
    ? Result<Trainee[]>
    : T extends "PORTAL_GET_FORM_OPTIONS"
      ? Result<TrainingFormOptions>
      : Result<unknown>;
