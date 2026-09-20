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

use crate::checkpoint::{BatchCheckpoint, CheckpointStatus, CheckpointWriter};
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
    /// Absent by default, mirroring `AutomationEngineOptions.checkpoint`: the
    /// unit tests want no storage, and a call site that forgets one gets a
    /// working engine that merely survives nothing. A real batch must supply
    /// one, or a run lost to a crash leaves no record of what it submitted.
    checkpoint: Option<Box<dyn CheckpointWriter>>,
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
            checkpoint: None,
        }
    }

    /// Attach the durable sink. Optional, and deliberately so — the TS engine's
    /// `checkpoint` option is optional for the same reason, and this is the
    /// builder form of it.
    pub fn with_checkpoint(mut self, checkpoint: Box<dyn CheckpointWriter>) -> Self {
        self.checkpoint = Some(checkpoint);

        self
    }

    /// Write the current position to durable storage.
    ///
    /// Failures are dropped on purpose. A storage error must not abort a batch
    /// that is otherwise succeeding — aborting would strand a trainee mid-flow,
    /// which is worse than a missing checkpoint, and the sink has already been
    /// told about it. Ports `saveCheckpoint` in AutomationEngine.ts.
    fn save_checkpoint(
        &mut self,
        status: CheckpointStatus,
        started_at: &str,
        total: usize,
        results: &[TrainingResult],
        queue: &TaskQueue<TraineeRef>,
    ) {
        let Some(sink) = self.checkpoint.as_mut() else {
            return;
        };

        let snapshot = BatchCheckpoint {
            status,
            // The report's `started_at` for this batch, so the two agree about
            // when the run began; `updated_at` is this write's own clock.
            started_at: started_at.to_string(),
            updated_at: now_ms(),
            total,
            results: results.to_vec(),
            pending: queue.ids(),
        };

        let _ = sink.write(&snapshot);
    }

    /// Run a batch: one shared session, one training template, many trainees.
    pub fn run<T: Transport>(
        &mut self,
        worker: &mut T,
        session: &SessionInput,
        trainees: Vec<TraineeRef>,
    ) -> Result<BatchReport, String> {
        let started_at = now_ms();
        // Taken from the queue as asked for, not from what survives the run:
        // `total` answers "how big was this batch", so a later skip-drain must
        // not shrink it.
        let total = trainees.len();
        self.machine.reset();
        let _ = self.machine.transition_to(AutomationState::Initializing);

        // Once per batch — a failure here aborts before any record is written.
        // Deliberately no checkpoint on this path: the TS engine writes a
        // terminal one with an untouched queue, which says "finished, nothing
        // attempted" about a batch that never started. The report on stderr is
        // the honest account of that.
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
            results.push(result);

            // Between the push and the abort check, exactly where the TS engine
            // puts it: this is the checkpoint that has to carry the result which
            // caused the abort, and the one a crash during the drain below would
            // leave behind.
            self.save_checkpoint(
                CheckpointStatus::Running,
                &started_at,
                total,
                &results,
                &queue,
            );

            let last = results.last().expect("just pushed");
            if should_abort(last) {
                let reason = abort_reason(last);
                drain_skipped(&mut queue, &mut results, &reason);
                let _ = self.machine.transition_to(AutomationState::Stopped);
                break;
            }
        }

        if self.machine.state() != AutomationState::Stopped {
            let _ = self.machine.transition_to(AutomationState::Complete);
        }

        // Terminal either way: an aborted batch is as final as a completed one,
        // and its results are the ones most worth keeping, since they say what
        // reached the portal before it went wrong. This is also the first
        // write to contain the drained skips — they are never checkpointed
        // individually, because a batch that reaches the drain has already
        // stopped submitting.
        self.save_checkpoint(
            CheckpointStatus::Finished,
            &started_at,
            total,
            &results,
            &queue,
        );

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
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use crate::checkpoint::{BatchCheckpoint, CheckpointStatus, CheckpointWriter};

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

    // -- checkpointing ------------------------------------------------------

    /// Records every checkpoint in order. The engine owns the sink, so the test
    /// keeps a handle rather than taking it back.
    #[derive(Clone, Default)]
    struct Recorder(Rc<RefCell<Vec<BatchCheckpoint>>>);

    impl Recorder {
        fn written(&self) -> Vec<BatchCheckpoint> {
            self.0.borrow().clone()
        }
    }

    impl CheckpointWriter for Recorder {
        fn write(&mut self, checkpoint: &BatchCheckpoint) -> Result<(), String> {
            self.0.borrow_mut().push(checkpoint.clone());

            Ok(())
        }
    }

    struct Failing;

    impl CheckpointWriter for Failing {
        fn write(&mut self, _: &BatchCheckpoint) -> Result<(), String> {
            Err("storage unavailable".to_string())
        }
    }

    fn with_recorder(recorder: &Recorder) -> BatchEngine {
        fast_engine().with_checkpoint(Box::new(recorder.clone()))
    }

    /// One write per settled trainee, then a terminal one. A single write at the
    /// end would mean a crash mid-batch leaves no record of what it already
    /// submitted — the gap this whole stage exists to close.
    #[test]
    fn checkpoints_after_every_settled_trainee() {
        let recorder = Recorder::default();
        let mut w = FakeTransport::new(
            true,
            vec![confirmed("1", "A"), confirmed("2", "B"), confirmed("3", "C")],
        );

        with_recorder(&recorder)
            .run(&mut w, &session(), refs(&["1", "2", "3"]))
            .unwrap();

        let written = recorder.written();
        let statuses: Vec<CheckpointStatus> = written.iter().map(|c| c.status).collect();
        let processed: Vec<usize> = written.iter().map(|c| c.results.len()).collect();

        assert_eq!(
            statuses,
            vec![
                CheckpointStatus::Running,
                CheckpointStatus::Running,
                CheckpointStatus::Running,
                CheckpointStatus::Finished,
            ]
        );
        assert_eq!(processed, vec![1, 2, 3, 3]);
    }

    /// The first checkpoint has to name the work that has not happened yet, or
    /// recovery cannot tell a trainee it never attempted from one it never saw.
    #[test]
    fn a_checkpoint_names_the_trainees_still_queued() {
        let recorder = Recorder::default();
        let mut w = FakeTransport::new(
            true,
            vec![confirmed("1", "A"), confirmed("2", "B"), confirmed("3", "C")],
        );

        with_recorder(&recorder)
            .run(&mut w, &session(), refs(&["1", "2", "3"]))
            .unwrap();

        let first = &recorder.written()[0];

        assert_eq!(first.status, CheckpointStatus::Running);
        assert_eq!(first.total, 3);
        assert_eq!(first.started_at, recorder.written().last().unwrap().started_at);
        assert_eq!(
            first
                .results
                .iter()
                .map(|r| r.trainee_id.as_str())
                .collect::<Vec<_>>(),
            vec!["1"]
        );
        assert_eq!(first.pending, vec!["2", "3"]);
    }

    #[test]
    fn the_terminal_checkpoint_has_nothing_pending() {
        let recorder = Recorder::default();
        let mut w = FakeTransport::new(true, vec![confirmed("1", "A"), confirmed("2", "B")]);

        with_recorder(&recorder)
            .run(&mut w, &session(), refs(&["1", "2"]))
            .unwrap();

        let last = recorder.written().pop().expect("a terminal write");

        assert_eq!(last.status, CheckpointStatus::Finished);
        assert!(last.pending.is_empty());
        assert_eq!(last.results.len(), 2);
    }

    /// The ordering that matters most: the checkpoint taken at the abort still
    /// has the whole queue pending, and the drain that follows is never
    /// checkpointed on its own. A crash in that window must leave the
    /// un-attempted trainees visible as unprocessed.
    #[test]
    fn the_abort_checkpoint_precedes_the_skip_drain() {
        let recorder = Recorder::default();
        let mut w = FakeTransport::new(
            true,
            vec![
                confirmed("1", "A"),
                resp(r#"{"v":1,"status":"ok","outcome":"indeterminate","message":"?","trainee":{"id":"2","name":"B"}}"#),
            ],
        );

        with_recorder(&recorder)
            .run(&mut w, &session(), refs(&["1", "2", "3"]))
            .unwrap();

        let written = recorder.written();
        let aborting = written
            .iter()
            .find(|c| c.results.iter().any(|r| r.outcome == Outcome::Indeterminate))
            .expect("the write that saw the abort");

        assert_eq!(aborting.status, CheckpointStatus::Running);
        assert_eq!(aborting.pending, vec!["3"]);
        assert!(
            written
                .iter()
                .filter(|c| c.status == CheckpointStatus::Running)
                .all(|c| c.results.iter().all(|r| r.outcome != Outcome::Skipped)),
            "skips belong to the terminal write only"
        );

        let last = written.last().unwrap();

        assert_eq!(last.status, CheckpointStatus::Finished);
        assert!(last.pending.is_empty());
        assert_eq!(last.results[2].outcome, Outcome::Skipped);
    }

    /// A batch that is submitting successfully must not be aborted by a storage
    /// fault; the sink reports its own failures.
    #[test]
    fn a_failing_checkpoint_writer_does_not_stop_the_batch() {
        let mut w = FakeTransport::new(true, vec![confirmed("1", "A"), confirmed("2", "B")]);

        let report = fast_engine()
            .with_checkpoint(Box::new(Failing))
            .run(&mut w, &session(), refs(&["1", "2"]))
            .unwrap();

        assert_eq!(report.successful, 2);
        assert_eq!(w.submit_calls, 2);
    }

    #[test]
    fn a_batch_runs_without_a_checkpoint_writer() {
        let mut w = FakeTransport::new(true, vec![confirmed("1", "A")]);

        let report = fast_engine().run(&mut w, &session(), refs(&["1"])).unwrap();

        assert_eq!(report.successful, 1);
    }

    /// The one test that proves the halves are wired to each other: a real
    /// [`CheckpointStore`] on a real disk, driven by the engine, read back by a
    /// second store — which is exactly what recovery will do.
    ///
    /// The recorder tests above and the store tests below prove each side in
    /// isolation, and both would stay green if nothing ever attached a sink to
    /// the engine at all, which is the state this stage found the tree in. This
    /// is the only test that would notice.
    #[test]
    fn a_batch_through_a_real_store_lands_on_disk() {
        use crate::store::CheckpointStore;

        let dir = std::env::temp_dir().join(format!("dssp-bot-engine-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("dssp.checkpoint.json");

        let mut w = FakeTransport::new(true, vec![confirmed("1", "A"), confirmed("2", "B")]);
        let report = fast_engine()
            .with_checkpoint(Box::new(CheckpointStore::new(&path)))
            .run(&mut w, &session(), refs(&["1", "2"]))
            .unwrap();

        assert_eq!(report.successful, 2);

        let restored = CheckpointStore::new(&path)
            .load()
            .expect("reads")
            .expect("the engine wrote a checkpoint");

        assert_eq!(restored.status, CheckpointStatus::Finished);
        assert_eq!(restored.total, 2);
        assert_eq!(restored.started_at, report.started_at);
        assert_eq!(restored.results.len(), 2);
        assert!(restored.pending.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
