/**
 * Decides whether the page's dialogs are currently suppressed.
 *
 * Split out of the MAIN-world script for two reasons: the decision is the
 * safety-critical part (getting it wrong means auto-accepting the portal's own
 * destructive prompts), and the script around it is untestable — it patches
 * globals at import time.
 *
 * Deliberately free of `chrome.*` and of `window`: the MAIN world has neither.
 */

/**
 * Longest a single arm request may last, whatever the caller asks for.
 *
 * A bound is required because the arm command arrives over `postMessage`, which
 * the page can also post to. An unbounded ttl would let a hostile page silence
 * its own confirmation prompts for the lifetime of the tab.
 */
export const MAX_ARM_MS = 60_000;

export class DialogGate {
  /** Epoch ms until which dialogs are suppressed. 0 means pass-through. */
  private armedUntil = 0;

  private readonly now: () => number;

  constructor(now: () => number = Date.now) {
    this.now = now;
  }

  /**
   * Suppression expires on a deadline rather than waiting to be switched off.
   *
   * If the ISOLATED half is torn down mid-batch — navigation, a killed service
   * worker, a thrown handler — nothing is left to send the disarm. The page has
   * to recover on its own, so the armed state carries its own expiry.
   */
  get armed(): boolean {
    return this.now() < this.armedUntil;
  }

  /**
   * Arm for `ttlMs`, clamped to `MAX_ARM_MS`.
   *
   * Returns the ttl actually applied, or 0 if the request was not a usable
   * duration — in which case nothing is armed. `ttlMs` is typed `unknown`
   * because it arrives as page-reachable input and cannot be trusted to be a
   * number at all.
   */
  arm(ttlMs: unknown): number {
    if (typeof ttlMs !== "number" || !Number.isFinite(ttlMs) || ttlMs <= 0) {
      return 0;
    }

    const applied = Math.min(ttlMs, MAX_ARM_MS);

    // Never shorten an arm that is already running: two overlapping submissions
    // must not have the first one's disarm cut the second one short.
    this.armedUntil = Math.max(this.armedUntil, this.now() + applied);

    return applied;
  }

  /** Hand dialogs back to the user. Idempotent. */
  disarm(): void {
    this.armedUntil = 0;
  }
}
