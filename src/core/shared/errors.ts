export class AutomationError extends Error {
  public readonly code: string;

  constructor(message: string, code: string) {
    super(message);
    this.name = "AutomationError";
    this.code = code;
  }
}

export class PortalElementNotFoundError extends AutomationError {
  constructor(element: string) {
    super(`Portal element not found: ${element}`, "ELEMENT_NOT_FOUND");
    this.name = "PortalElementNotFoundError";
  }
}

export class SessionExpiredError extends AutomationError {
  constructor() {
    super("The DSSP session has expired.", "SESSION_EXPIRED");
    this.name = "SessionExpiredError";
  }
}

export class SubmissionError extends AutomationError {
  constructor(message: string) {
    super(message, "SUBMISSION_FAILED");
    this.name = "SubmissionError";
  }
}