import { describe, expect, it } from "vitest";
import { RetryPolicy, isRecoverable } from "../../src/core/shared/retry";
import {
  NetworkError,
  SessionExpiredError,
  TimeoutError,
  DuplicateRecordError,
} from "../../src/core/shared/errors";

const immediate = {
  initialDelayMs: 0,
  maxDelayMs: 0,
};

describe("isRecoverable", () => {
  it("marks transient portal failures recoverable", () => {
    expect(isRecoverable(new NetworkError("offline"))).toBe(true);
    expect(isRecoverable(new TimeoutError("submit", 100))).toBe(true);
  });

  it("marks terminal failures non-recoverable", () => {
    expect(isRecoverable(new SessionExpiredError())).toBe(false);
    expect(isRecoverable(new DuplicateRecordError("exists"))).toBe(false);
    expect(isRecoverable(new Error("plain"))).toBe(false);
  });
});

describe("RetryPolicy", () => {
  it("returns the first successful attempt", async () => {
    const policy = new RetryPolicy(immediate);
    let calls = 0;

    const result = await policy.execute(() => {
      calls += 1;

      return Promise.resolve("ok");
    });

    expect(result).toBe("ok");
    expect(calls).toBe(1);
  });

  it("retries recoverable failures up to maxAttempts", async () => {
    const policy = new RetryPolicy({
      ...immediate,
      maxAttempts: 3,
    });

    let calls = 0;

    const result = await policy.execute((attempt) => {
      calls += 1;

      if (attempt < 3) {
        return Promise.reject(new NetworkError("temporary"));
      }

      return Promise.resolve("recovered");
    });

    expect(result).toBe("recovered");
    expect(calls).toBe(3);
  });

  it("does not retry non-recoverable failures", async () => {
    const policy = new RetryPolicy({
      ...immediate,
      maxAttempts: 5,
    });

    let calls = 0;

    await expect(
      policy.execute(() => {
        calls += 1;

        return Promise.reject(new SessionExpiredError());
      }),
    ).rejects.toBeInstanceOf(SessionExpiredError);

    expect(calls).toBe(1);
  });

  it("throws the last error once attempts are exhausted", async () => {
    const policy = new RetryPolicy({
      ...immediate,
      maxAttempts: 2,
    });

    let calls = 0;

    await expect(
      policy.execute(() => {
        calls += 1;

        return Promise.reject(new NetworkError("down"));
      }),
    ).rejects.toBeInstanceOf(NetworkError);

    expect(calls).toBe(2);
  });

  it("caps exponential backoff at maxDelayMs", () => {
    const policy = new RetryPolicy({
      initialDelayMs: 100,
      backoffFactor: 2,
      maxDelayMs: 300,
    });

    expect(policy.delayForAttempt(1)).toBe(100);
    expect(policy.delayForAttempt(2)).toBe(200);
    expect(policy.delayForAttempt(3)).toBe(300);
    expect(policy.delayForAttempt(9)).toBe(300);
  });
});
