export type ErrorCode =
  | "ELEMENT_NOT_FOUND"
  | "TIMEOUT"
  | "NETWORK"
  | "PORTAL_UNAVAILABLE"
  | "SESSION_EXPIRED"
  | "TRAINEE_NOT_FOUND"
  | "MISSING_DATA"
  | "VALIDATION_FAILED"
  | "DUPLICATE_RECORD"
  | "PORTAL_STRUCTURE_CHANGED"
  | "PORTAL_NOT_MAPPED"
  | "CONFIRMATION_UNKNOWN"
  | "SUBMISSION_FAILED";

export class AutomationError extends Error {
  readonly code: ErrorCode;
  readonly recoverable: boolean;

  constructor(message: string, code: ErrorCode, recoverable: boolean) {
    super(message);
    this.name = "AutomationError";
    this.code = code;
    this.recoverable = recoverable;
  }
}

export class PortalElementNotFoundError extends AutomationError {
  constructor(element: string) {
    super(`Portal element not found: ${element}`, "ELEMENT_NOT_FOUND", true);
    this.name = "PortalElementNotFoundError";
  }
}

export class TimeoutError extends AutomationError {
  constructor(operation: string, timeoutMs: number) {
    super(`Timed out after ${timeoutMs}ms: ${operation}`, "TIMEOUT", true);
    this.name = "TimeoutError";
  }
}

export class NetworkError extends AutomationError {
  constructor(message: string) {
    super(message, "NETWORK", true);
    this.name = "NetworkError";
  }
}

export class PortalUnavailableError extends AutomationError {
  constructor(message: string) {
    super(message, "PORTAL_UNAVAILABLE", true);
    this.name = "PortalUnavailableError";
  }
}

export class SessionExpiredError extends AutomationError {
  constructor() {
    super("The DSSP session has expired.", "SESSION_EXPIRED", false);
    this.name = "SessionExpiredError";
  }
}

export class TraineeNotFoundError extends AutomationError {
  constructor(traineeId: string) {
    super(`Trainee not found: ${traineeId}`, "TRAINEE_NOT_FOUND", false);
    this.name = "TraineeNotFoundError";
  }
}

export class MissingDataError extends AutomationError {
  constructor(field: string) {
    super(`Required training data is missing: ${field}`, "MISSING_DATA", false);
    this.name = "MissingDataError";
  }
}

export class ValidationError extends AutomationError {
  constructor(message: string) {
    super(message, "VALIDATION_FAILED", false);
    this.name = "ValidationError";
  }
}

export class DuplicateRecordError extends AutomationError {
  constructor(message: string) {
    super(message, "DUPLICATE_RECORD", false);
    this.name = "DuplicateRecordError";
  }
}

export class PortalStructureError extends AutomationError {
  constructor(message: string) {
    super(message, "PORTAL_STRUCTURE_CHANGED", false);
    this.name = "PortalStructureError";
  }
}

export class PortalNotMappedError extends AutomationError {
  constructor(operation: string) {
    super(
      `Portal operation "${operation}" is not mapped yet. Complete portal discovery before running automation.`,
      "PORTAL_NOT_MAPPED",
      false,
    );
    this.name = "PortalNotMappedError";
  }
}

export class SubmissionError extends AutomationError {
  constructor(message: string) {
    super(message, "SUBMISSION_FAILED", false);
    this.name = "SubmissionError";
  }
}

/**
 * The form was submitted but the portal never gave a usable answer, so whether
 * the training record exists is unknown. Deliberately non-recoverable: retrying
 * would mean submitting a second time.
 */
export class ConfirmationUnknownError extends AutomationError {
  constructor(message: string) {
    super(
      `Submitted, but the result could not be confirmed: ${message}`,
      "CONFIRMATION_UNKNOWN",
      false,
    );
    this.name = "ConfirmationUnknownError";
  }
}

export function toAutomationError(error: unknown): AutomationError {
  if (error instanceof AutomationError) {
    return error;
  }

  const message = error instanceof Error ? error.message : String(error);

  return new AutomationError(message, "SUBMISSION_FAILED", false);
}
