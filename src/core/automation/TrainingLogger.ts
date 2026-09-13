import { Logger } from "../infrastructure/logging/Logger";
import type { AutomationError } from "../shared/errors";

export type TrainingAction =
  | "batch_start"
  | "batch_complete"
  | "load_trainee"
  | "open_form"
  | "fill_form"
  | "validate_form"
  | "submit_form"
  | "verify_result"
  | "retry"
  | "pause"
  | "resume"
  | "stop";

export type ActionStatus = "started" | "succeeded" | "failed" | "skipped";

export interface TrainingLogEntry {
  timestamp: string;
  action: TrainingAction;
  status: ActionStatus;
  traineeId?: string;
  traineeName?: string;
  attempt?: number;
  errorCode?: string;
  errorMessage?: string;
}

export interface TrainingLogContext {
  traineeId?: string;
  traineeName?: string;
  attempt?: number;
}

export class TrainingLogger {
  private readonly logger: Logger;
  private readonly entries: TrainingLogEntry[] = [];

  constructor(logger: Logger = new Logger("DSSP")) {
    this.logger = logger;
  }

  record(
    action: TrainingAction,
    status: ActionStatus,
    context: TrainingLogContext = {},
    error?: AutomationError,
  ): TrainingLogEntry {
    const entry: TrainingLogEntry = {
      timestamp: new Date().toISOString(),
      action,
      status,
      ...context,
      ...(error === undefined
        ? {}
        : {
            errorCode: error.code,
            errorMessage: error.message,
          }),
    };

    this.entries.push(entry);

    const message = `${action}:${status}`;

    if (status === "failed") {
      this.logger.error(message, { ...entry });
    } else {
      this.logger.info(message, { ...entry });
    }

    return entry;
  }

  getEntries(): readonly TrainingLogEntry[] {
    return this.entries;
  }

  getEntriesForTrainee(traineeId: string): TrainingLogEntry[] {
    return this.entries.filter((entry) => entry.traineeId === traineeId);
  }

  clear(): void {
    this.entries.length = 0;
  }
}
