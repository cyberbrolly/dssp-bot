import type {
  DaemonReport,
  DaemonCheckpoint,
  TraineeRef,
} from "../daemon/DaemonClient";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { AutomationState } from "../../automation/AutomationState";
import type { TrainingFormOptions } from "../../domain/TrainingFormOptions";

export interface BatchProgress {
  state: AutomationState;
  total: number;
  processed: number;
  successful: number;
  failed: number;
  skipped: number;
  /** Submitted but unconfirmed. Each one needs manual verification. */
  indeterminate: number;
  remaining: number;
  currentTraineeName?: string;
  /** Daemon intent only; paused is published after the safe checkpoint boundary. */
  pauseRequested?: boolean;
  /** Latched daemon intent; stopped is published only by the engine. */
  stopRequested?: boolean;
}

export type Message =
  | { type: "GET_STATUS" }
  | { type: "GET_FORM_OPTIONS" }
  | {
      type: "START_AUTOMATION";
      traineeIds: string[];
      trainees?: TraineeRef[];
      resume?: boolean;
      session: Omit<TrainingSession, "traineeId">;
    }
  | { type: "PAUSE_AUTOMATION" }
  | { type: "RESUME_AUTOMATION" }
  | { type: "STOP_AUTOMATION" }
  | { type: "GET_REPORT" }
  | { type: "GET_CHECKPOINT" };

export type MessageType = Message["type"];

export type MessageResponse =
  { success: true; data?: unknown } | { success: false; error: string };

export interface StatusResponse {
  success: true;
  data: BatchProgress;
}

export interface ReportResponse {
  success: true;
  data: DaemonReport | null;
}

export interface CheckpointResponse {
  success: true;
  data: DaemonCheckpoint | null;
}

export interface FormOptionsResponse {
  success: true;
  data: TrainingFormOptions;
}
