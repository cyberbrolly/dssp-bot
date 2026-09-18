//! Retry / stop policy. Rust owns every decision; the worker only reports.
//!
//! This is the safety core ported from AutomationEngine.ts + retry.ts:
//! an indeterminate submission is never retried, a failure that might have
//! reached the portal is treated as indeterminate, and only a provably-safe,
//! transient error is retried (bounded, with backoff).

use std::time::Duration;

use crate::protocol::{Response, Status};

#[derive(Debug, Clone)]
pub enum Decision {
    /// Definitive, success-side result (confirmed / duplicate). Report and stop.
    Accept(String),
    /// Definitive failure a retry cannot fix (rejected / bad input / no match).
    Reject(String),
    /// Stop and require a human: submitted-but-unconfirmed, a maybe-landed
    /// transport failure, an expired session, or a changed portal.
    Halt(String),
    /// Provably nothing submitted and transient — safe to try again.
    Retry(String),
}

pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_factor: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // Mirrors DEFAULT_RETRY_OPTIONS in src/core/shared/retry.ts.
        Self {
            max_attempts: 3,
            initial_delay_ms: 500,
            max_delay_ms: 5_000,
            backoff_factor: 2,
        }
    }
}

impl RetryPolicy {
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let raw = self
            .initial_delay_ms
            .saturating_mul(self.backoff_factor.saturating_pow(attempt.saturating_sub(1)));
        Duration::from_millis(raw.min(self.max_delay_ms))
    }
}

/// Codes that a retry can plausibly clear (mirror of AutomationError.recoverable).
fn is_recoverable(code: &str) -> bool {
    matches!(
        code,
        "ELEMENT_NOT_FOUND" | "TIMEOUT" | "NETWORK" | "PORTAL_UNAVAILABLE"
    )
}

/// Codes that must stop the whole batch (mirror of shouldAbortBatch).
fn aborts_batch(code: &str) -> bool {
    matches!(code, "SESSION_EXPIRED" | "PORTAL_STRUCTURE_CHANGED")
}

/// Decide what to do with a `submit_training` response.
pub fn decide_submit(resp: &Response) -> Decision {
    match resp.status {
        Status::Ok => match resp.outcome.as_deref() {
            Some("confirmed") => Decision::Accept(match &resp.reference {
                Some(r) => format!("confirmed (ref {r})"),
                None => "confirmed".to_string(),
            }),
            Some("duplicate") => Decision::Accept(format!(
                "duplicate — {}",
                resp.message.as_deref().unwrap_or("already recorded")
            )),
            Some("rejected") => Decision::Reject(format!(
                "rejected — {}",
                resp.message.as_deref().unwrap_or("portal refused the record")
            )),
            Some("indeterminate") => Decision::Halt(format!(
                "indeterminate — {} (submitted but unconfirmed; verify on the portal before re-running)",
                resp.message.as_deref().unwrap_or("no readable result")
            )),
            other => Decision::Halt(format!("unrecognized outcome {other:?}")),
        },
        Status::Error => {
            let code = resp.error_code.as_deref().unwrap_or("SUBMISSION_FAILED");
            let proves_nothing = resp.proves_nothing_submitted.unwrap_or(false);
            let message = resp.message.as_deref().unwrap_or("");

            if aborts_batch(code) {
                Decision::Halt(format!("{code} — {message}"))
            } else if !proves_nothing {
                // NETWORK/TIMEOUT/etc.: the POST may have landed. Never retry.
                Decision::Halt(format!(
                    "{code} — {message} (submission may have landed; not retrying)"
                ))
            } else if is_recoverable(code) {
                Decision::Retry(format!("{code} — {message}"))
            } else {
                // Provably nothing submitted, but a retry won't help
                // (validation, missing data, no/ambiguous trainee, bad request).
                Decision::Reject(format!("{code} — {message}"))
            }
        }
        Status::Ready => Decision::Halt("unexpected ready line mid-flow".to_string()),
    }
}

/// Batch-oriented settling of a `submit_training` response into a per-trainee
/// result category. Where [`decide_submit`] answers "what should the operator
/// see", this answers "what goes in the batch report / should we retry",
/// mirroring the outcome mapping in AutomationEngine.ts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settle {
    Success,
    Failed { code: String, message: String },
    Indeterminate { message: String },
    Retry { code: String, message: String },
}

pub fn settle_submit(resp: &Response) -> Settle {
    match resp.status {
        Status::Ok => match resp.outcome.as_deref() {
            Some("confirmed") => Settle::Success,
            Some("duplicate") => Settle::Failed {
                code: "DUPLICATE_RECORD".to_string(),
                message: resp
                    .message
                    .clone()
                    .unwrap_or_else(|| "already recorded".to_string()),
            },
            Some("rejected") => Settle::Failed {
                code: "SUBMISSION_FAILED".to_string(),
                message: resp
                    .message
                    .clone()
                    .unwrap_or_else(|| "portal refused the record".to_string()),
            },
            Some("indeterminate") => Settle::Indeterminate {
                message: resp
                    .message
                    .clone()
                    .unwrap_or_else(|| "no readable result".to_string()),
            },
            other => Settle::Indeterminate {
                message: format!("unrecognized outcome {other:?}"),
            },
        },
        Status::Error => {
            let code = resp
                .error_code
                .clone()
                .unwrap_or_else(|| "SUBMISSION_FAILED".to_string());
            let proves_nothing = resp.proves_nothing_submitted.unwrap_or(false);
            let message = resp.message.clone().unwrap_or_default();

            if proves_nothing && !aborts_batch(&code) && is_recoverable(&code) {
                Settle::Retry { code, message }
            } else if !proves_nothing {
                // May have reached the portal — treat as indeterminate.
                Settle::Indeterminate {
                    message: format!("{code} — {message} (submission may have landed)"),
                }
            } else {
                // Provably nothing submitted; a retry won't help. Session/structure
                // codes flow through here and trigger the batch abort downstream.
                Settle::Failed { code, message }
            }
        }
        Status::Ready => Settle::Indeterminate {
            message: "unexpected ready line mid-flow".to_string(),
        },
    }
}
