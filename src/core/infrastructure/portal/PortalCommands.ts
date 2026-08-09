import type { Result } from "../../shared/Result";
import type { Trainee } from "../../domain/Trainee";
import type { TrainingSession } from "../../domain/TrainingSession";

export type PortalCommand =
  | { type: "PORTAL_IS_PAGE" }
  | { type: "PORTAL_GET_TRAINEES" }
  | { type: "PORTAL_OPEN_TRAINEE"; trainee: Trainee }
  | { type: "PORTAL_OPEN_FORM" }
  | { type: "PORTAL_FILL_FORM"; session: TrainingSession }
  | { type: "PORTAL_VALIDATE_FORM" }
  | { type: "PORTAL_SUBMIT_FORM" }
  | { type: "PORTAL_WAIT_RESULT" };

export type PortalCommandType = PortalCommand["type"];

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
  T extends "PORTAL_GET_TRAINEES" ? Result<Trainee[]> : Result<unknown>;
