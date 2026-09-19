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

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(json: &str) -> Response {
        serde_json::from_str(json).expect("valid response json")
    }

    fn ok(body: &str) -> Response {
        resp(&format!(r#"{{"v":1,"status":"ok",{body}}}"#))
    }

    fn err(body: &str) -> Response {
        resp(&format!(r#"{{"v":1,"status":"error",{body}}}"#))
    }

    // -- decide_submit: the single-trainee live path ------------------------

    #[test]
    fn confirmed_is_accepted_and_carries_its_reference() {
        match decide_submit(&ok(r#""outcome":"confirmed","reference":"REF-1""#)) {
            Decision::Accept(s) => assert!(s.contains("REF-1"), "{s}"),
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[test]
    fn confirmed_without_a_reference_is_still_accepted() {
        match decide_submit(&ok(r#""outcome":"confirmed""#)) {
            Decision::Accept(s) => assert_eq!(s, "confirmed"),
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    /// A duplicate is a success-side result: the record exists.
    #[test]
    fn duplicate_is_accepted() {
        match decide_submit(&ok(r#""outcome":"duplicate","message":"already logged""#)) {
            Decision::Accept(s) => assert!(s.starts_with("duplicate"), "{s}"),
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[test]
    fn rejected_is_a_definitive_failure() {
        match decide_submit(&ok(r#""outcome":"rejected","message":"bad date""#)) {
            Decision::Reject(s) => assert!(s.contains("bad date"), "{s}"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn indeterminate_halts_and_never_retries() {
        match decide_submit(&ok(r#""outcome":"indeterminate","message":"no result""#)) {
            Decision::Halt(s) => assert!(s.contains("verify on the portal"), "{s}"),
            other => panic!("expected Halt, got {other:?}"),
        }
    }

    /// An outcome we do not recognize must not be treated as success.
    #[test]
    fn unrecognized_outcome_halts() {
        assert!(matches!(
            decide_submit(&ok(r#""outcome":"exploded""#)),
            Decision::Halt(_)
        ));
    }

    #[test]
    fn session_expired_halts_the_job() {
        match decide_submit(&err(
            r#""error_code":"SESSION_EXPIRED","message":"gone","proves_nothing_submitted":true"#,
        )) {
            Decision::Halt(s) => assert!(s.contains("SESSION_EXPIRED"), "{s}"),
            other => panic!("expected Halt, got {other:?}"),
        }
    }

    #[test]
    fn portal_structure_change_halts_the_job() {
        assert!(matches!(
            decide_submit(&err(
                r#""error_code":"PORTAL_STRUCTURE_CHANGED","proves_nothing_submitted":true"#
            )),
            Decision::Halt(_)
        ));
    }

    /// The safety core: an error that may have reached the portal is never
    /// retried, however transient the code looks.
    #[test]
    fn a_maybe_landed_error_halts_instead_of_retrying() {
        match decide_submit(&err(
            r#""error_code":"NETWORK","message":"reset","proves_nothing_submitted":false"#,
        )) {
            Decision::Halt(s) => assert!(s.contains("may have landed"), "{s}"),
            other => panic!("expected Halt, got {other:?}"),
        }
    }

    /// An absent flag means "unknown", never "safe to retry".
    #[test]
    fn a_missing_proves_nothing_flag_is_treated_as_maybe_landed() {
        assert!(matches!(
            decide_submit(&err(r#""error_code":"NETWORK","message":"reset""#)),
            Decision::Halt(_)
        ));
    }

    #[test]
    fn a_provably_unsent_transient_error_retries() {
        match decide_submit(&err(
            r#""error_code":"ELEMENT_NOT_FOUND","message":"no field","proves_nothing_submitted":true"#,
        )) {
            Decision::Retry(s) => assert!(s.contains("ELEMENT_NOT_FOUND"), "{s}"),
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn a_provably_unsent_permanent_error_rejects_without_retrying() {
        match decide_submit(&err(
            r#""error_code":"TRAINEE_NOT_FOUND","message":"no match","proves_nothing_submitted":true"#,
        )) {
            Decision::Reject(s) => assert!(s.contains("TRAINEE_NOT_FOUND"), "{s}"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn a_ready_line_mid_flow_halts() {
        assert!(matches!(
            decide_submit(&resp(r#"{"v":1,"status":"ready"}"#)),
            Decision::Halt(_)
        ));
    }

    // -- settle_submit: the batch path --------------------------------------

    #[test]
    fn settle_confirmed_is_success() {
        assert_eq!(
            settle_submit(&ok(r#""outcome":"confirmed""#)),
            Settle::Success
        );
    }

    /// A duplicate fails this trainee but must not abort the batch.
    #[test]
    fn settle_duplicate_is_a_failure_that_does_not_abort() {
        match settle_submit(&ok(r#""outcome":"duplicate","message":"already logged""#)) {
            Settle::Failed { code, message } => {
                assert_eq!(code, "DUPLICATE_RECORD");
                assert!(message.contains("already logged"), "{message}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn settle_rejected_is_a_failure() {
        match settle_submit(&ok(r#""outcome":"rejected","message":"bad""#)) {
            Settle::Failed { code, .. } => assert_eq!(code, "SUBMISSION_FAILED"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn settle_indeterminate_is_indeterminate() {
        assert!(matches!(
            settle_submit(&ok(r#""outcome":"indeterminate","message":"?""#)),
            Settle::Indeterminate { .. }
        ));
    }

    /// Where decide_submit halts, the batch path records indeterminate so the
    /// report names the trainee a human has to check.
    #[test]
    fn settle_unrecognized_outcome_is_indeterminate() {
        assert!(matches!(
            settle_submit(&ok(r#""outcome":"exploded""#)),
            Settle::Indeterminate { .. }
        ));
    }

    #[test]
    fn settle_maybe_landed_error_is_indeterminate_not_retry() {
        assert!(matches!(
            settle_submit(&err(
                r#""error_code":"NETWORK","message":"reset","proves_nothing_submitted":false"#
            )),
            Settle::Indeterminate { .. }
        ));
    }

    #[test]
    fn settle_provably_unsent_transient_error_retries() {
        match settle_submit(&err(
            r#""error_code":"TIMEOUT","message":"slow","proves_nothing_submitted":true"#,
        )) {
            Settle::Retry { code, .. } => assert_eq!(code, "TIMEOUT"),
            other => panic!("expected Retry, got {other:?}"),
        }
    }

    /// A session/portal error settles as Failed rather than Retry even though
    /// it proves nothing was submitted — the batch abort reads the error code
    /// from exactly this result.
    #[test]
    fn settle_session_expired_fails_rather_than_retrying() {
        match settle_submit(&err(
            r#""error_code":"SESSION_EXPIRED","message":"gone","proves_nothing_submitted":true"#,
        )) {
            Settle::Failed { code, .. } => assert_eq!(code, "SESSION_EXPIRED"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn settle_ready_line_is_indeterminate() {
        assert!(matches!(
            settle_submit(&resp(r#"{"v":1,"status":"ready"}"#)),
            Settle::Indeterminate { .. }
        ));
    }

    // -- retry policy -------------------------------------------------------

    #[test]
    fn backoff_grows_then_caps_at_the_ceiling() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for_attempt(1).as_millis(), 500);
        assert_eq!(policy.delay_for_attempt(2).as_millis(), 1_000);
        assert_eq!(policy.delay_for_attempt(3).as_millis(), 2_000);
        assert_eq!(policy.delay_for_attempt(4).as_millis(), 4_000);
        // 8_000 ms would exceed the ceiling.
        assert_eq!(policy.delay_for_attempt(5).as_millis(), 5_000);
        // Exponential growth must saturate, not overflow.
        assert_eq!(policy.delay_for_attempt(50).as_millis(), 5_000);
    }

    #[test]
    fn attempt_zero_does_not_underflow() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for_attempt(0).as_millis(), 500);
    }
}
