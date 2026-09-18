//! Batch coordinator. Ports the orchestration core of AutomationEngine.ts:
//! establish the session once, run each trainee through the retry policy, and
//! abort the whole batch the moment a result cannot be accounted for (an
//! indeterminate submission, an expired session, or a changed portal), draining
//! everything still queued as skipped.
//!
//! The worker is reached through the [`Transport`] trait so the engine is
//! testable without a subprocess or a browser.

use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::decision::{settle_submit, RetryPolicy, Settle};
use crate::protocol::{Request, Response, SessionInput, Status, TraineeRef};
use crate::queue::TaskQueue;
use crate::report::{BatchReport, Outcome, TrainingResult};
use crate::state::{AutomationState, StateMachine};

/// Anything that can exchange one protocol message with the worker.
pub trait Transport {
    fn send(&mut self, req: &Request) -> Result<Response, String>;
}

impl Transport for crate::worker::WorkerClient {
    fn send(&mut self, req: &Request) -> Result<Response, String> {
        crate::worker::WorkerClient::send(self, req)
    }
}

fn now_ms() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub struct BatchEngine {
    policy: RetryPolicy,
    machine: StateMachine,
}

impl Default for BatchEngine {
    fn default() -> Self {
        Self::with_policy(RetryPolicy::default())
    }
}

impl BatchEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_policy(policy: RetryPolicy) -> Self {
        Self {
            policy,
            machine: StateMachine::new(),
        }
    }

    /// Run a batch: one shared session, one training template, many trainees.
    pub fn run<T: Transport>(
        &mut self,
        worker: &mut T,
        session: &SessionInput,
        trainees: Vec<TraineeRef>,
    ) -> Result<BatchReport, String> {
        let started_at = now_ms();
        self.machine.reset();
        let _ = self.machine.transition_to(AutomationState::Initializing);

        // Once per batch — a failure here aborts before any record is written.
        let ensure = worker.send(&Request::ensure_session(new_id()))?;
        if ensure.status != Status::Ok {
            return Err(format!("session not established: {}", ensure.summary()));
        }

        let mut queue: TaskQueue<TraineeRef> = TaskQueue::new();
        for (i, trainee) in trainees.into_iter().enumerate() {
            let id = trainee
                .id
                .clone()
                .or_else(|| trainee.name.clone())
                .unwrap_or_else(|| format!("#{i}"));
            queue.enqueue(id, trainee);
        }

        let mut results: Vec<TrainingResult> = Vec::new();

        while let Some(task) = queue.dequeue() {
            let result = self.process_trainee(worker, session, &task.payload);
            let abort = should_abort(&result);
            results.push(result);

            if abort {
                let reason = abort_reason(results.last().expect("just pushed"));
                drain_skipped(&mut queue, &mut results, &reason);
                let _ = self.machine.transition_to(AutomationState::Stopped);
                break;
            }
        }

        if self.machine.state() != AutomationState::Stopped {
            let _ = self.machine.transition_to(AutomationState::Complete);
        }

        Ok(BatchReport::build(results, started_at, now_ms()))
    }

    /// One trainee: submit with the retry policy, never re-committing an
    /// outcome that isn't provably safe to repeat.
    fn process_trainee<T: Transport>(
        &mut self,
        worker: &mut T,
        session: &SessionInput,
        trainee: &TraineeRef,
    ) -> TrainingResult {
        let _ = self.machine.transition_to(AutomationState::LoadingTrainee);
        let job_id = new_id();
        let fallback_id = trainee.id.clone().unwrap_or_default();
        let fallback_name = trainee.name.clone().unwrap_or_default();
        let mut attempts = 0u32;

        loop {
            attempts += 1;
            let _ = self.machine.transition_to(AutomationState::Submitting);

            let req = Request::submit_training(job_id.clone(), trainee.clone(), session.clone());
            let resp = match worker.send(&req) {
                Ok(resp) => resp,
                Err(e) => {
                    // Transport failure mid-submit: state unknown, treat as indeterminate.
                    return TrainingResult {
                        trainee_id: fallback_id,
                        trainee_name: fallback_name,
                        outcome: Outcome::Indeterminate,
                        attempts,
                        error_code: Some("NETWORK".to_string()),
                        message: Some(format!("transport error: {e}")),
                    };
                }
            };

            let (id, name) = resp
                .trainee
                .as_ref()
                .map(|t| (t.id.clone(), t.name.clone()))
                .unwrap_or_else(|| (fallback_id.clone(), fallback_name.clone()));

            match settle_submit(&resp) {
                Settle::Success => {
                    let _ = self.machine.transition_to(AutomationState::Verifying);
                    return TrainingResult {
                        trainee_id: id,
                        trainee_name: name,
                        outcome: Outcome::Success,
                        attempts,
                        error_code: None,
                        message: resp.reference.clone(),
                    };
                }
                Settle::Failed { code, message } => {
                    let _ = self.machine.transition_to(AutomationState::Verifying);
                    return TrainingResult {
                        trainee_id: id,
                        trainee_name: name,
                        outcome: Outcome::Failed,
                        attempts,
                        error_code: Some(code),
                        message: Some(message),
                    };
                }
                Settle::Indeterminate { message } => {
                    let _ = self.machine.transition_to(AutomationState::Verifying);
                    return TrainingResult {
                        trainee_id: id,
                        trainee_name: name,
                        outcome: Outcome::Indeterminate,
                        attempts,
                        error_code: resp.error_code.clone(),
                        message: Some(message),
                    };
                }
                Settle::Retry { code, message } => {
                    if attempts >= self.policy.max_attempts {
                        let _ = self.machine.transition_to(AutomationState::Verifying);
                        return TrainingResult {
                            trainee_id: id,
                            trainee_name: name,
                            outcome: Outcome::Failed,
                            attempts,
                            error_code: Some(code),
                            message: Some(format!("gave up after {attempts} attempts: {message}")),
                        };
                    }
                    let _ = self.machine.transition_to(AutomationState::Retrying);
                    thread::sleep(self.policy.delay_for_attempt(attempts));
                }
            }
        }
    }
}

/// An unconfirmed submission, an expired session, or a changed portal stops the
/// whole batch — mirror of shouldAbortBatch.
fn should_abort(result: &TrainingResult) -> bool {
    result.outcome == Outcome::Indeterminate
        || matches!(
            result.error_code.as_deref(),
            Some("SESSION_EXPIRED") | Some("PORTAL_STRUCTURE_CHANGED")
        )
}

fn abort_reason(result: &TrainingResult) -> String {
    if result.outcome == Outcome::Indeterminate {
        let who = if result.trainee_name.is_empty() {
            &result.trainee_id
        } else {
            &result.trainee_name
        };
        format!(
            "batch aborted: {who} was submitted but could not be confirmed — \
             verify on the portal before re-running"
        )
    } else {
        format!(
            "batch aborted: {}",
            result.error_code.as_deref().unwrap_or("unrecoverable error")
        )
    }
}

fn drain_skipped(
    queue: &mut TaskQueue<TraineeRef>,
    results: &mut Vec<TrainingResult>,
    reason: &str,
) {
    while let Some(task) = queue.dequeue() {
        let trainee = task.payload;
        results.push(TrainingResult {
            trainee_id: trainee.id.clone().unwrap_or_default(),
            trainee_name: trainee.name.clone().unwrap_or_default(),
            outcome: Outcome::Skipped,
            attempts: 0,
            error_code: None,
            message: Some(reason.to_string()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripts one response per submit_training call, in order.
    struct FakeTransport {
        ensure_ok: bool,
        submits: VecDeque<Response>,
        submit_calls: usize,
    }

    impl FakeTransport {
        fn new(ensure_ok: bool, submits: Vec<Response>) -> Self {
            Self {
                ensure_ok,
                submits: submits.into(),
                submit_calls: 0,
            }
        }
    }

    impl Transport for FakeTransport {
        fn send(&mut self, req: &Request) -> Result<Response, String> {
            match req.op {
                crate::protocol::op::ENSURE_SESSION => Ok(resp(if self.ensure_ok {
                    r#"{"v":1,"status":"ok","authenticated":true}"#
                } else {
                    r#"{"v":1,"status":"error","error_code":"SESSION_EXPIRED","message":"login","proves_nothing_submitted":true}"#
                })),
                crate::protocol::op::SUBMIT_TRAINING => {
                    self.submit_calls += 1;
                    Ok(self
                        .submits
                        .pop_front()
                        .expect("no scripted submit response left"))
                }
                other => panic!("unexpected op {other}"),
            }
        }
    }

    fn resp(json: &str) -> Response {
        serde_json::from_str(json).expect("valid response json")
    }

    fn confirmed(id: &str, name: &str) -> Response {
        resp(&format!(
            r#"{{"v":1,"status":"ok","outcome":"confirmed","reference":"u","trainee":{{"id":"{id}","name":"{name}"}},"attempts":1}}"#
        ))
    }

    fn session() -> SessionInput {
        SessionInput {
            training_date: "2026-09-14".to_string(),
            instructor: "10".to_string(),
            training_type: "1".to_string(),
        }
    }

    fn refs(ids: &[&str]) -> Vec<TraineeRef> {
        ids.iter()
            .map(|id| TraineeRef {
                id: Some((*id).to_string()),
                name: None,
            })
            .collect()
    }

    fn fast_engine() -> BatchEngine {
        // Zero delays keep retry tests instant.
        BatchEngine::with_policy(RetryPolicy {
            max_attempts: 3,
            initial_delay_ms: 0,
            max_delay_ms: 0,
            backoff_factor: 2,
        })
    }

    #[test]
    fn all_confirmed() {
        let mut w = FakeTransport::new(
            true,
            vec![confirmed("1", "A"), confirmed("2", "B"), confirmed("3", "C")],
        );
        let report = fast_engine().run(&mut w, &session(), refs(&["1", "2", "3"])).unwrap();
        assert_eq!((report.total, report.successful, report.failed), (3, 3, 0));
        assert_eq!(report.skipped, 0);
        assert_eq!(w.submit_calls, 3);
        assert_eq!(report.success_rate, 1.0);
    }

    #[test]
    fn indeterminate_aborts_and_drains_rest_as_skipped() {
        let mut w = FakeTransport::new(
            true,
            vec![
                confirmed("1", "A"),
                resp(r#"{"v":1,"status":"ok","outcome":"indeterminate","message":"?","trainee":{"id":"2","name":"B"}}"#),
                // third submit must never happen
            ],
        );
        let report = fast_engine().run(&mut w, &session(), refs(&["1", "2", "3"])).unwrap();
        assert_eq!(report.total, 3);
        assert_eq!(report.successful, 1);
        assert_eq!(report.indeterminate, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(w.submit_calls, 2, "must not submit after an abort");
        assert_eq!(report.results[2].outcome, Outcome::Skipped);
    }

    #[test]
    fn rejected_and_duplicate_fail_but_batch_continues() {
        let mut w = FakeTransport::new(
            true,
            vec![
                resp(r#"{"v":1,"status":"ok","outcome":"rejected","message":"bad","trainee":{"id":"1","name":"A"}}"#),
                confirmed("2", "B"),
                resp(r#"{"v":1,"status":"ok","outcome":"duplicate","message":"already logged","trainee":{"id":"3","name":"C"}}"#),
            ],
        );
        let report = fast_engine().run(&mut w, &session(), refs(&["1", "2", "3"])).unwrap();
        assert_eq!((report.total, report.successful, report.failed, report.skipped), (3, 1, 2, 0));
        assert_eq!(w.submit_calls, 3);
    }

    #[test]
    fn element_not_found_retries_then_succeeds() {
        let mut w = FakeTransport::new(
            true,
            vec![
                resp(r#"{"v":1,"status":"error","error_code":"ELEMENT_NOT_FOUND","message":"no field","proves_nothing_submitted":true}"#),
                confirmed("1", "A"),
            ],
        );
        let report = fast_engine().run(&mut w, &session(), refs(&["1"])).unwrap();
        assert_eq!(report.successful, 1);
        assert_eq!(report.results[0].attempts, 2);
        assert_eq!(w.submit_calls, 2);
    }

    #[test]
    fn session_expired_aborts_batch() {
        let mut w = FakeTransport::new(
            true,
            vec![resp(
                r#"{"v":1,"status":"error","error_code":"SESSION_EXPIRED","message":"gone","proves_nothing_submitted":true}"#,
            )],
        );
        let report = fast_engine().run(&mut w, &session(), refs(&["1", "2"])).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.results[0].error_code.as_deref(), Some("SESSION_EXPIRED"));
        assert_eq!(w.submit_calls, 1);
    }

    #[test]
    fn ensure_session_failure_stops_before_any_submit() {
        let mut w = FakeTransport::new(false, vec![]);
        let err = fast_engine()
            .run(&mut w, &session(), refs(&["1"]))
            .unwrap_err();
        assert!(err.contains("session"), "{err}");
        assert_eq!(w.submit_calls, 0);
    }
}
