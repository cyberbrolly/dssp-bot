import { delay } from "./delay";
import { AutomationError } from "./errors";

export interface RetryOptions {
  maxAttempts: number;
  initialDelayMs: number;
  maxDelayMs: number;
  backoffFactor: number;
}

export const DEFAULT_RETRY_OPTIONS: RetryOptions = {
  maxAttempts: 3,
  initialDelayMs: 500,
  maxDelayMs: 5_000,
  backoffFactor: 2,
};

export function isRecoverable(error: unknown): boolean {
  return error instanceof AutomationError ? error.recoverable : false;
}

export class RetryPolicy {
  private readonly options: RetryOptions;

  constructor(options: Partial<RetryOptions> = {}) {
    this.options = {
      ...DEFAULT_RETRY_OPTIONS,
      ...options,
    };
  }

  get maxAttempts(): number {
    return this.options.maxAttempts;
  }

  delayForAttempt(attempt: number): number {
    const raw =
      this.options.initialDelayMs * this.options.backoffFactor ** (attempt - 1);

    return Math.min(raw, this.options.maxDelayMs);
  }

  async execute<T>(
    operation: (attempt: number) => Promise<T>,
    signal?: AbortSignal,
  ): Promise<T> {
    let lastError: unknown = new Error("Retry policy executed zero attempts.");

    for (let attempt = 1; attempt <= this.options.maxAttempts; attempt += 1) {
      try {
        return await operation(attempt);
      } catch (error) {
        lastError = error;

        if (!isRecoverable(error) || attempt === this.options.maxAttempts) {
          throw error;
        }

        await delay(this.delayForAttempt(attempt), signal);
      }
    }

    throw lastError;
  }
}
