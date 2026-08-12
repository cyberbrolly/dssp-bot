import type { BatchReport } from "../../domain/BatchReport";
import type { BatchCheckpoint } from "../../automation/BatchCheckpoint";
import type { TrainingSession } from "../../domain/TrainingSession";
import type { AutomationState } from "../../automation/AutomationState";

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
}

export type Message =
  | { type: "GET_STATUS" }
  | {
      type: "START_AUTOMATION";
      traineeIds: string[];
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
  data: BatchReport | null;
}

export interface CheckpointResponse {
  success: true;
  data: BatchCheckpoint | null;
}
