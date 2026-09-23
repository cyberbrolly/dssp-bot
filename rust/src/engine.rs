//! Batch coordinator. Ports the orchestration core of AutomationEngine.ts:
//! establish the session once, run each trainee through the retry policy, and
//! abort the whole batch the moment a result cannot be accounted for (an
//! indeterminate submission, an expired session, or a changed portal), draining
//! everything still queued as skipped.
//!
//! The worker is reached through the [`Transport`] trait so the engine is
//! testable without a subprocess or a browser.

use std::collections::HashSet;
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

/// The key a trainee is queued under, and the one written to `in_flight`.
///
/// One definition for both, because they have to agree: whatever `run` records
/// as in flight is what a resume will look for in the queue. Id first, since a
/// resume re-resolves what it finds in the file against the portal and a name is
/// not an identity there. The positional fallback exists only so an entry
/// carrying neither is still addressable in the queue; `resolve_against` gives
/// the CLI an id before it reaches here, so it is a library-caller case.
fn queue_key(trainee: &TraineeRef, index: usize) -> String {
    trainee
        .id
        .clone()
        .or_else(|| trainee.name.clone())
        .unwrap_or_else(|| format!("#{index}"))
}

/// A roster is a set: collapse repeats, keep the first occurrence.
///
/// Two entries under one key would submit the same trainee twice, and after a
/// crash the second copy is still in `pending` — where a resume would run it
/// again and re-submit something already confirmed. `resolve_against` collapses
/// repeats on the CLI path before the engine sees them; this is the same rule
/// applied at the last point before a submit, so a caller that skips that
/// pre-flight cannot reach the worker with a repeat either.
///
/// Later entries lose, matching `resolve_against`'s "already queued — skipping",
/// and the caller must count the result rather than its input: a dropped repeat
/// left inside `total` would put the file permanently one trainee short of its
/// groups, which `blocks_start` reads as a submission that may have gone
/// unrecorded and refuses on every start thereafter.
fn dedupe(trainees: Vec<TraineeRef>) -> Vec<(String, TraineeRef)> {
    let mut seen: HashSet<String> = HashSet::with_capacity(trainees.len());
    let mut unique: Vec<(String, TraineeRef)> = Vec::with_capacity(trainees.len());

    for (i, trainee) in trainees.into_iter().enumerate() {
        let key = queue_key(&trainee, i);

        if !seen.insert(key.clone()) {
            eprintln!("dssp-bot: {key} is already queued — skipping the repeat");

            continue;
        }

        unique.push((key, trainee));
    }

    unique
}

pub struct BatchEngine {
    policy: RetryPolicy,
    machine: StateMachine,
    /// Where the durable record of this batch goes.
    ///
    /// Not an `Option`, and not a builder step, so it cannot be left out. A
    /// batch that submits without a sink puts records on the portal that nothing
    /// on disk names, and the next start reads that silence as "never sent" and
    /// submits them again — the failure this stage exists to prevent.
    /// `AutomationEngineOptions.checkpoint` is optional; this is deliberately
    /// narrower, for the same reason Stage 28 narrows that engine's
    /// swallow-everything `saveCheckpoint` rather than keeping it. The unit tests
    /// pass a recorder; the CLI passes [`crate::store::CheckpointStore`].
    checkpoint: Box<dyn CheckpointWriter>,
    /// Results a predecessor could not account for, carried into this batch.
    ///
    /// History, not work: they are never queued, never re-decided and never
    /// allowed to abort the run. They are here because the checkpoint file is
    /// rewritten in full, so the first write of a resumed batch would otherwise
    /// drop the only record of a submission that may already exist.
    carried: Vec<TrainingResult>,
}

impl BatchEngine {
    /// An engine with the default retry policy and the given sink.
    pub fn new(checkpoint: Box<dyn CheckpointWriter>) -> Self {
        Self::with_policy(RetryPolicy::default(), checkpoint)
    }

    /// The full constructor: the policy this batch runs under, and where its
    /// durable record goes. Both are required, which is the point — neither is
    /// something a caller can usefully omit.
    pub fn with_policy(policy: RetryPolicy, checkpoint: Box<dyn CheckpointWriter>) -> Self {
        Self {
            policy,
            machine: StateMachine::new(),
            checkpoint,
            carried: Vec::new(),
        }
    }

    /// Seed the run with records a previous batch could not settle.
    pub fn with_carried(mut self, carried: Vec<TrainingResult>) -> Self {
        self.carried = carried;

        self
    }

    /// Write the current position to durable storage.
    ///
    /// The returned error is the caller's to use, and there is exactly one call
    /// that uses it: the write taken before a submission, which `run` treats as
    /// fatal. Every other one is dropped, because a storage error must not abort
    /// a batch that is otherwise succeeding — aborting would strand a trainee
    /// mid-flow, which is worse than a missing checkpoint, and the sink has
    /// already been told about it. Ports `saveCheckpoint` in AutomationEngine.ts,
    /// whose swallow-everything behaviour Stage 28 narrows rather than keeps;
    /// see the pre-submission call in [`BatchEngine::run`] for why that one
    /// write is different.
    fn save_checkpoint(
        &mut self,
        status: CheckpointStatus,
        started_at: &str,
        total: usize,
        results: &[TrainingResult],
        queue: &TaskQueue<TraineeRef>,
        in_flight: Option<&str>,
    ) -> Result<(), String> {
        let snapshot = BatchCheckpoint {
            status,
            // The report's `started_at` for this batch, so the two agree about
            // when the run began; `updated_at` is this write's own clock.
            started_at: started_at.to_string(),
            updated_at: now_ms(),
            total,
            results: results.to_vec(),
            pending: queue.ids(),
            in_flight: in_flight.map(str::to_string),
        };

        self.checkpoint.write(&snapshot)
    }

    /// Run a batch: one shared session, one training template, many trainees.
    pub fn run<T: Transport>(
        &mut self,
        worker: &mut T,
        session: &SessionInput,
        trainees: Vec<TraineeRef>,
    ) -> Result<BatchReport, String> {
        let started_at = now_ms();
        // Whatever a predecessor could not settle is seeded first: it belongs to
        // this file from its very first write, or the write that follows would
        // replace the only record of a submission that may already exist.
        let mut results: Vec<TrainingResult> = std::mem::take(&mut self.carried);
        // Collapsed before the count is taken, never after: `total` has to agree
        // with the groups a reader will sum, and a dropped repeat that was still
        // counted would leave the file one trainee short of itself.
        let queued = dedupe(trainees);
        // Taken from the queue as asked for, not from what survives the run:
        // `total` answers "how many trainees this file accounts for", so a later
        // skip-drain must not shrink it — and the carried records are part of
        // that accounting, which is what keeps each trainee in exactly one group.
        let total = queued.len() + results.len();
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
        for (key, trainee) in queued {
            queue.enqueue(key, trainee);
        }

        while let Some(task) = queue.dequeue() {
            // Before the submission, not after it. From this instant the trainee
            // is in no other group — it has left the queue and its result does
            // not exist yet — so if the process dies now, this write is the only
            // thing that can say a submission may have reached the portal. Its
            // window spans the whole portal round trip and every retry backoff.
            //
            // The one checkpoint write whose failure stops the run. Everywhere
            // else a storage error is survivable because a later write follows
            // it; here no later write can undo the send that would come next. A
            // run that cannot record the window must not open it: submitting
            // anyway would put a trainee on the portal with nothing durable
            // naming it, and the next start would read that silence as "never
            // sent" and submit it a second time — the exact duplicate this stage
            // exists to prevent. Not sending strands nothing, so the usual
            // argument against aborting (a trainee caught mid-flow) does not
            // apply.
            if let Err(e) = self.save_checkpoint(
                CheckpointStatus::Running,
                &started_at,
                total,
                &results,
                &queue,
                Some(&task.id),
            ) {
                eprintln!(
                    "dssp-bot: stopping the batch before submitting {}: its checkpoint could not \
                     be written: {e}",
                    task.id
                );
                // Back into the queue so the drain below records it as skipped
                // — never sent, and so safe to run again. Dropping it here
                // instead would leave it in no group at all, and a resume would
                // not know it still owes a submission.
                queue.put_back(task);
                drain_skipped(&mut queue, &mut results, NOT_SUBMITTED);
                let _ = self.machine.transition_to(AutomationState::Stopped);
                break;
            }

            let result = self.process_trainee(worker, session, &task.payload);
            results.push(result);

            // Cleared by the same write that records the result, so no
            // checkpoint ever lists one trainee under both.
            //
            // Between the abort check and this write is where the TS engine
            // puts its own: this is the checkpoint that has to carry the result
            // which caused the abort, and the one a crash during the drain below
            // would leave behind.
            let _ = self.save_checkpoint(
                CheckpointStatus::Running,
                &started_at,
                total,
                &results,
                &queue,
                None,
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
        let _ = self.save_checkpoint(
            CheckpointStatus::Finished,
            &started_at,
            total,
            &results,
            &queue,
            None,
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

/// Why the trainees a run stopped before reaching are recorded as skipped.
///
/// Distinct from an abort's reason on purpose: an abort means the portal refused
/// something, and this means the run never asked. Both leave a trainee that is
/// safe to run again, which is what `Skipped` promises, but only the second one
/// is a local fault the operator can clear.
const NOT_SUBMITTED: &str = "not submitted: the run stopped before reaching it";

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
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    use crate::checkpoint::{BatchCheckpoint, CheckpointStatus, CheckpointWriter};

    /// Scripts one response per submit_training call, in order.
    ///
    /// It also doubles as the instrument for the crash-window tests: given a
    /// sink handle it snapshots what that sink had been given at the instant
    /// each submission was sent, which is the only place the ordering between
    /// "recorded" and "sent" can actually be observed. `die_on_call` turns one
    /// of those instants into a process death.
    struct FakeTransport {
        ensure_ok: bool,
        submits: VecDeque<Response>,
        submit_calls: usize,
        probe: Option<Rc<RefCell<Vec<BatchCheckpoint>>>>,
        seen_at_send: Vec<Option<BatchCheckpoint>>,
        die_on_call: Option<usize>,
    }

    impl FakeTransport {
        fn new(ensure_ok: bool, submits: Vec<Response>) -> Self {
            Self {
                ensure_ok,
                submits: submits.into(),
                submit_calls: 0,
                probe: None,
                seen_at_send: Vec::new(),
                die_on_call: None,
            }
        }

        fn watching(ensure_ok: bool, submits: Vec<Response>, recorder: &Recorder) -> Self {
            Self {
                probe: Some(recorder.handle()),
                ..Self::new(ensure_ok, submits)
            }
        }

        /// What the sink held when submission `n` (1-based) was sent.
        fn at_send(&self, n: usize) -> &BatchCheckpoint {
            self.seen_at_send
                .get(n - 1)
                .unwrap_or_else(|| panic!("submission {n} was never sent"))
                .as_ref()
                .unwrap_or_else(|| panic!("nothing was written before submission {n}"))
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

                    if let Some(probe) = self.probe.clone() {
                        let seen = probe.borrow().last().cloned();
                        self.seen_at_send.push(seen);
                    }

                    if self.die_on_call == Some(self.submit_calls) {
                        panic!("simulated crash: the coordinator died mid-submission");
                    }

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

    /// Zero delays keep retry tests instant.
    fn fast_policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            initial_delay_ms: 0,
            max_delay_ms: 0,
            backoff_factor: 2,
        }
    }

    /// An engine whose sink the test never reads back: the policy is what these
    /// tests are about, so the writes land in a recorder nobody holds a handle
    /// to. Every engine has somewhere to write — that is the constructor's whole
    /// point — so "no sink" is no longer a state a test can ask for.
    fn fast_engine() -> BatchEngine {
        BatchEngine::with_policy(fast_policy(), Box::new(Recorder::default()))
    }

    /// The same, writing to a sink the test can read.
    fn with_recorder(recorder: &Recorder) -> BatchEngine {
        BatchEngine::with_policy(fast_policy(), Box::new(recorder.clone()))
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

    /// A roster is a set. The same id twice would be two submissions, the second
    /// of which the portal refuses as a duplicate and the report counts as a
    /// failure — a batch that went fine reading as one that needed attention.
    /// `resolve_against` collapses repeats before the CLI gets here; this is the
    /// same rule at the last point before a submit.
    #[test]
    fn a_repeated_id_is_queued_and_submitted_once() {
        // Three responses for three queued entries, so a run that submits the
        // repeat reaches the assertions below rather than dying on an exhausted
        // script: a regression here should say "three submits, not two", not
        // "no scripted submit response left". The third goes unused.
        let mut w = FakeTransport::new(
            true,
            vec![confirmed("1", "A"), confirmed("1", "A"), confirmed("2", "B")],
        );

        let report = fast_engine()
            .run(&mut w, &session(), refs(&["1", "1", "2"]))
            .unwrap();

        assert_eq!(w.submit_calls, 2, "the repeat never reached the worker");
        assert_eq!(report.total, 2, "and the file accounts for two, not three");
        assert_eq!(report.successful, 2);
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

        /// The shared sink, for a test that needs to look inside it at a moment
        /// the engine is not between two calls.
        fn handle(&self) -> Rc<RefCell<Vec<BatchCheckpoint>>> {
            Rc::clone(&self.0)
        }
    }

    impl CheckpointWriter for Recorder {
        fn write(&mut self, checkpoint: &BatchCheckpoint) -> Result<(), String> {
            self.0.borrow_mut().push(checkpoint.clone());

            Ok(())
        }
    }

    /// A sink that accepts `ok` writes and then fails, the way a disk fills up
    /// mid-batch while the run keeps going. Shared like [`Recorder`], because
    /// the engine takes ownership of the sink and the test still needs to read
    /// what reached it.
    #[derive(Clone)]
    struct Filling {
        ok: Rc<Cell<usize>>,
        seen: Rc<RefCell<Vec<BatchCheckpoint>>>,
    }

    impl Filling {
        fn new(ok: usize) -> Self {
            Self {
                ok: Rc::new(Cell::new(ok)),
                seen: Rc::new(RefCell::new(Vec::new())),
            }
        }

        /// What the run managed to persist before the storage gave out.
        fn persisted(&self) -> Vec<BatchCheckpoint> {
            self.seen.borrow().clone()
        }
    }

    impl CheckpointWriter for Filling {
        fn write(&mut self, checkpoint: &BatchCheckpoint) -> Result<(), String> {
            let left = self.ok.get();

            if left == 0 {
                return Err("no space left on device".to_string());
            }

            self.ok.set(left - 1);
            self.seen.borrow_mut().push(checkpoint.clone());

            Ok(())
        }
    }

    /// The store-backed engine: the real sink, on a real path.
    fn with_store(path: &std::path::Path) -> BatchEngine {
        BatchEngine::with_policy(fast_policy(), Box::new(crate::store::CheckpointStore::new(path)))
    }

    /// One settled write per trainee, then a terminal one. A single write at the
    /// end would mean a crash mid-batch leaves no record of what it already
    /// submitted — the gap this whole stage exists to close.
    ///
    /// The writes taken *before* each submission are the in-flight markers, and
    /// they are the subject of the tests below; this one is about the settled
    /// record, so it reads only the writes that have no trainee in flight.
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

        let settled: Vec<BatchCheckpoint> = recorder
            .written()
            .into_iter()
            .filter(|c| c.in_flight.is_none())
            .collect();
        let statuses: Vec<CheckpointStatus> = settled.iter().map(|c| c.status).collect();
        let processed: Vec<usize> = settled.iter().map(|c| c.results.len()).collect();

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

        let written = recorder.written();
        let first = written
            .iter()
            .find(|c| c.in_flight.is_none())
            .expect("a settled write");

        assert_eq!(first.status, CheckpointStatus::Running);
        assert_eq!(first.total, 3);
        assert_eq!(first.started_at, written.last().unwrap().started_at);
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

    // -- the crash window ---------------------------------------------------

    /// The ordering this stage exists for, observed from inside the window: at
    /// the instant the submission is sent, the sink already names the trainee
    /// as in flight, and the queue no longer does.
    ///
    /// Fails against the code before this stage, which wrote nothing until the
    /// result came back — leaving the trainee in no group at all for the length
    /// of the portal round trip.
    #[test]
    fn a_trainee_is_checkpointed_before_its_submission_is_sent() {
        let recorder = Recorder::default();
        let mut w = FakeTransport::watching(
            true,
            vec![confirmed("1", "A"), confirmed("2", "B"), confirmed("3", "C")],
            &recorder,
        );

        with_recorder(&recorder)
            .run(&mut w, &session(), refs(&["1", "2", "3"]))
            .unwrap();

        let first = w.at_send(1);
        assert_eq!(first.in_flight.as_deref(), Some("1"));
        assert!(first.results.is_empty(), "{first:?}");
        assert_eq!(first.pending, vec!["2", "3"]);

        let second = w.at_send(2);
        assert_eq!(second.in_flight.as_deref(), Some("2"));
        assert_eq!(second.results.len(), 1, "only trainee 1 had settled");
    }

    /// The window made concrete: the process dies between the marker and the
    /// result, and the record the next process reads still names the trainee
    /// whose submission may have reached the portal.
    #[test]
    fn a_crash_mid_submission_leaves_the_trainee_in_flight() {
        let recorder = Recorder::default();
        // Held outside the closure: the engine that would have owned the sink is
        // gone by the time the assertions run.
        let observed = recorder.handle();
        let mut w = FakeTransport::watching(true, vec![confirmed("1", "A")], &recorder);
        w.die_on_call = Some(2);

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            with_recorder(&recorder).run(&mut w, &session(), refs(&["1", "2", "3"]))
        }));

        assert!(crashed.is_err(), "the simulated crash must propagate");

        let written = observed.borrow().clone();
        let last = written.last().expect("the write taken before the second send");

        assert_eq!(last.status, CheckpointStatus::Running);
        assert_eq!(last.in_flight.as_deref(), Some("2"));
        assert_eq!(last.results.len(), 1, "trainee 1 settled before the crash");
        assert_eq!(last.pending, vec!["3"]);

        let split = last.unreconciled();
        assert_eq!(split.indeterminate.len(), 1, "the crashed trainee is unconfirmed");
        assert_eq!(split.indeterminate[0].trainee_id, "2");
        assert!(!split.unprocessed.contains(&"2".to_string()), "and not merely queued");
    }

    /// Gate 3's evidence, end to end and on a real disk: a batch killed in the
    /// commit window, the file a later process finds, and the decision that file
    /// produces.
    ///
    /// The test above proves the write ordering inside one process. This one
    /// proves the bytes reached the disk and that a *second* store, opening the
    /// file the way the CLI does, refuses the next batch by default and then —
    /// under the acknowledgement — continues without ever re-queuing the trainee
    /// whose submission may already exist. Every link in that chain is a place
    /// the guarantee could be lost silently.
    #[test]
    fn a_crash_on_disk_is_refused_and_its_trainee_is_never_requeued() {
        use crate::recovery::{self, Plan};
        use crate::store::CheckpointStore;

        let dir = std::env::temp_dir().join(format!("dssp-bot-crash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("dssp.checkpoint.json");

        let mut w = FakeTransport::new(
            true,
            vec![confirmed("1", "A"), confirmed("2", "B"), confirmed("3", "C")],
        );
        w.die_on_call = Some(2);

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_store(&path)
                .run(&mut w, &session(), refs(&["1", "2", "3"]))
        }));

        assert!(crashed.is_err(), "the simulated crash must propagate");
        assert_eq!(w.submit_calls, 2, "the process died inside the second submit");

        // What the next process finds when it opens the same file.
        let store = CheckpointStore::new(&path);
        let found = store.load().expect("reads").expect("the crash left a file");

        assert_eq!(
            found.in_flight.as_deref(),
            Some("2"),
            "the trainee that was being submitted is named on disk"
        );
        assert_eq!(found.pending, vec!["3"], "and the one behind it was not dequeued");

        // Refused by default: a new batch's first write would replace the only
        // account of a submission that may already exist.
        let refusal = recovery::guard(&store, false).expect_err("must not be overwritten");
        assert!(refusal.contains("killed while submitting 2"), "{refusal}");

        // And under the acknowledgement, trainee 2 is carried rather than run.
        let start = recovery::guard(&store, true).expect("the override");

        match start.plan {
            Plan::Resume { roster, carried, .. } => {
                let ids: Vec<String> = roster.iter().filter_map(|r| r.id.clone()).collect();

                assert_eq!(ids, vec!["3"], "only the trainee that was never sent");
                assert!(carried.iter().any(|r| r.trainee_id == "2"));
                assert!(
                    carried.iter().any(|r| r.trainee_id == "1" && r.outcome == Outcome::Success),
                    "and the run it did record is not lost either"
                );
            }
            other => panic!("expected a resume, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A trainee is never in two groups at once: the write that records a
    /// result is the write that clears the marker.
    #[test]
    fn the_in_flight_marker_is_cleared_once_the_trainee_settles() {        let recorder = Recorder::default();
        let mut w = FakeTransport::new(true, vec![confirmed("1", "A"), confirmed("2", "B")]);

        with_recorder(&recorder)
            .run(&mut w, &session(), refs(&["1", "2"]))
            .unwrap();

        for checkpoint in recorder.written() {
            if let Some(id) = &checkpoint.in_flight {
                assert!(
                    !checkpoint.results.iter().any(|r| &r.trainee_id == id),
                    "settled and in flight at once: {checkpoint:?}"
                );
            }
        }

        assert!(recorder.written().last().unwrap().in_flight.is_none());
    }

    /// One marker per trainee, not per attempt: every retry backoff sits inside
    /// the window the single pre-send write opens.
    #[test]
    fn a_retry_attempt_does_not_write_a_second_in_flight_record() {
        let recorder = Recorder::default();
        let mut w = FakeTransport::new(
            true,
            vec![
                resp(r#"{"v":1,"status":"error","error_code":"ELEMENT_NOT_FOUND","message":"no field","proves_nothing_submitted":true}"#),
                confirmed("1", "A"),
            ],
        );

        with_recorder(&recorder)
            .run(&mut w, &session(), refs(&["1"]))
            .unwrap();

        let marked: Vec<BatchCheckpoint> = recorder
            .written()
            .into_iter()
            .filter(|c| c.in_flight.is_some())
            .collect();

        assert_eq!(marked.len(), 1, "{marked:?}");
        assert_eq!(marked[0].in_flight.as_deref(), Some("1"));
        assert_eq!(w.submit_calls, 2, "the retry still happened");
    }

    /// The invariant the start gate and the recovery split both rest on, walked
    /// across every write of a batch that ends in an abort — including the
    /// drain, which is where the two halves of the accounting could disagree.
    #[test]
    fn every_checkpoint_accounts_for_every_trainee() {
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
        assert!(written.len() >= 4, "one marker per attempt plus the settled writes");

        for checkpoint in &written {
            let accounted = checkpoint.results.len()
                + checkpoint.pending.len()
                + usize::from(checkpoint.in_flight.is_some());

            assert_eq!(accounted, checkpoint.total, "{checkpoint:?}");
        }
    }

    // -- carried history ----------------------------------------------------

    fn indeterminate(id: &str, name: &str) -> TrainingResult {
        TrainingResult {
            trainee_id: id.to_string(),
            trainee_name: name.to_string(),
            outcome: Outcome::Indeterminate,
            attempts: 1,
            error_code: Some("NETWORK".to_string()),
            message: Some("no confirmation".to_string()),
        }
    }

    /// A carried record belongs to the resumed batch's very first write, or the
    /// single checkpoint slot would drop the only account of a submission that
    /// may already exist on the portal.
    #[test]
    fn carried_results_survive_the_first_write_of_a_resumed_batch() {
        let recorder = Recorder::default();
        let mut w = FakeTransport::new(true, vec![confirmed("2", "B")]);

        with_recorder(&recorder)
            .with_carried(vec![indeterminate("1", "A")])
            .run(&mut w, &session(), refs(&["2"]))
            .unwrap();

        let first = recorder.written().into_iter().next().expect("a first write");

        assert_eq!(first.results.len(), 1);
        assert_eq!(first.results[0].trainee_id, "1");
        assert_eq!(first.total, 2, "the file accounts for both batches' work");
        assert_eq!(first.in_flight.as_deref(), Some("2"));
    }

    /// History, not work: a carried record is never offered to the worker again.
    #[test]
    fn a_resumed_batch_never_resubmits_a_carried_record() {
        let mut w = FakeTransport::new(true, vec![confirmed("2", "B")]);

        let report = fast_engine()
            .with_carried(vec![indeterminate("1", "A")])
            .run(&mut w, &session(), refs(&["2"]))
            .unwrap();

        assert_eq!(w.submit_calls, 1, "only the trainee still to do");
        assert_eq!(report.total, 2);
        assert_eq!(report.successful, 1);
        assert_eq!(report.indeterminate, 1, "the unconfirmed record is still reported");
    }

    /// The carried record is the reason the previous batch stopped, so it must
    /// not stop the next one — only a result this run produced may abort it.
    #[test]
    fn a_carried_indeterminate_does_not_abort_the_resumed_batch() {
        let mut w = FakeTransport::new(true, vec![confirmed("2", "B"), confirmed("3", "C")]);

        let report = fast_engine()
            .with_carried(vec![indeterminate("1", "A")])
            .run(&mut w, &session(), refs(&["2", "3"]))
            .unwrap();

        assert_eq!(report.successful, 2);
        assert_eq!(report.skipped, 0, "the batch ran to the end");
        assert_eq!(w.submit_calls, 2);
    }

    /// Carried history is consumed by the run that picked it up; a second run of
    /// the same engine must not report the previous batch's records again.
    #[test]
    fn a_second_run_does_not_re_carry_the_first_runs_history() {
        let mut engine = fast_engine().with_carried(vec![indeterminate("1", "A")]);

        let mut first = FakeTransport::new(true, vec![confirmed("2", "B")]);
        assert_eq!(engine.run(&mut first, &session(), refs(&["2"])).unwrap().total, 2);

        let mut second = FakeTransport::new(true, vec![confirmed("3", "C")]);
        let report = engine.run(&mut second, &session(), refs(&["3"])).unwrap();

        assert_eq!(report.total, 1);
        assert_eq!(report.indeterminate, 0);
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
    ///
    /// The faults here start after the first trainee has been settled, which is
    /// the case this guarantee is about: a write that follows a submission,
    /// where a later write can still record what the loss would have hidden. The
    /// write taken *before* a submission is the exception, and has its own tests
    /// below.
    #[test]
    fn a_failing_checkpoint_writer_does_not_stop_the_batch() {
        let mut w = FakeTransport::new(true, vec![confirmed("1", "A"), confirmed("2", "B")]);
        // Four writes precede the one that fails: the in-flight and settled
        // writes for trainee 1, then the in-flight write for trainee 2 — so the
        // fault lands after both trainees were submitted, which is the case this
        // guarantee is about.
        let sink = Filling::new(3);

        let report = BatchEngine::with_policy(fast_policy(), Box::new(sink.clone()))
            .run(&mut w, &session(), refs(&["1", "2"]))
            .unwrap();

        assert_eq!(report.successful, 2);
        assert_eq!(w.submit_calls, 2);
        assert!(!sink.persisted().is_empty(), "the earlier writes landed");
    }

    /// The one write whose failure has to stop the run.
    ///
    /// If the in-flight record cannot be taken, the trainee must not be
    /// submitted: the file would keep saying it was never sent, and the next
    /// start would submit it again — a duplicate caused by a full disk rather
    /// than by a crash. Nothing was sent, so nothing is stranded, and the
    /// trainee is left in the group a resume knows how to pick up.
    #[test]
    fn a_failed_in_flight_write_stops_the_batch_before_submitting() {
        let mut w = FakeTransport::new(true, vec![confirmed("1", "A"), confirmed("2", "B")]);
        let sink = Filling::new(0);

        let report = BatchEngine::with_policy(fast_policy(), Box::new(sink.clone()))
            .run(&mut w, &session(), refs(&["1", "2"]))
            .unwrap();

        assert_eq!(w.submit_calls, 0, "nothing may reach the portal unrecorded");
        assert_eq!(report.skipped, 2);
        assert_eq!(report.total, 2, "both are accounted for, neither ran");
        assert!(sink.persisted().is_empty());
    }

    /// And the trainee it stopped on is *first* in the group, in the order the
    /// job queued it — so a resume re-runs the batch in the operator's order and
    /// not the order a failed write happened to leave behind.
    #[test]
    fn a_failed_in_flight_write_leaves_the_queue_in_order() {
        let mut w = FakeTransport::new(true, vec![confirmed("1", "A"), confirmed("2", "B")]);
        // One settled write for trainee 1, then the storage dies on trainee 2's
        // in-flight write.
        let sink = Filling::new(2);

        let report = BatchEngine::with_policy(fast_policy(), Box::new(sink.clone()))
            .run(&mut w, &session(), refs(&["1", "2", "3"]))
            .unwrap();

        assert_eq!(w.submit_calls, 1, "only trainee 1 was sent");
        assert_eq!(report.successful, 1);

        let ids: Vec<String> = report
            .results
            .iter()
            .filter(|r| r.outcome == Outcome::Skipped)
            .map(|r| r.trainee_id.clone())
            .collect();

        assert_eq!(ids, vec!["2", "3"], "the queue order, not the failure's");

        // The trainee that caused the stop is the one a resume would run first,
        // and the one that did run is not in that group at all.
        let last = sink.persisted().last().cloned().expect("a final write");
        assert!(last.never_attempted().contains(&"2".to_string()));
        assert!(!last.never_attempted().contains(&"1".to_string()));
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
        let report = with_store(&path)
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

    // ---- probe: crash matrix vs. Gate 3 (temporary, review scaffolding) ----

    struct Probe {
        sent: Rc<RefCell<Vec<String>>>,
        die_at: Option<usize>,
        calls: usize,
    }

    impl Transport for Probe {
        fn send(&mut self, req: &Request) -> Result<Response, String> {
            match req.op {
                crate::protocol::op::ENSURE_SESSION => Ok(resp(
                    r#"{"v":1,"status":"ok","authenticated":true}"#,
                )),
                crate::protocol::op::SUBMIT_TRAINING => {
                    self.calls += 1;
                    let id = req
                        .trainee
                        .as_ref()
                        .and_then(|t| t.id.clone())
                        .unwrap_or_default();
                    self.sent.borrow_mut().push(id.clone());
                    if self.die_at == Some(self.calls) {
                        panic!("simulated crash: killed while submitting {id}");
                    }
                    Ok(resp(&format!(
                        r#"{{"v":1,"status":"ok","outcome":"confirmed","reference":"u","trainee":{{"id":"{id}","name":"N{id}"}},"attempts":1}}"#
                    )))
                }
                other => panic!("unexpected op {other}"),
            }
        }
    }

    struct CrashSink {
        store: crate::store::CheckpointStore,
        writes: Rc<Cell<usize>>,
        die_before: Option<usize>,
        die_after: Option<usize>,
    }

    impl CheckpointWriter for CrashSink {
        fn write(&mut self, checkpoint: &BatchCheckpoint) -> Result<(), String> {
            let n = self.writes.get() + 1;
            self.writes.set(n);

            if self.die_before == Some(n) {
                panic!("simulated crash before write {n}");
            }

            let result = self.store.save(checkpoint);

            if self.die_after == Some(n) {
                panic!("simulated crash after write {n}");
            }

            result
        }
    }

    fn probe_one(
        die_write: Option<(usize, bool)>,
        die_submit: Option<usize>,
        trainees: &[&str],
    ) -> (Vec<String>, Option<BatchCheckpoint>) {
        use crate::store::CheckpointStore;

        let dir = std::env::temp_dir().join(format!("dssp-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("dssp.checkpoint.json");

        let sent = Rc::new(RefCell::new(Vec::new()));
        let mut transport = Probe {
            sent: Rc::clone(&sent),
            die_at: die_submit,
            calls: 0,
        };

        let (die_before, die_after) = match die_write {
            Some((n, false)) => (Some(n), None),
            Some((n, true)) => (None, Some(n)),
            None => (None, None),
        };

        let sink = CrashSink {
            store: CheckpointStore::new(&path),
            writes: Rc::new(Cell::new(0)),
            die_before,
            die_after,
        };

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            BatchEngine::with_policy(fast_policy(), Box::new(sink))
                .run(&mut transport, &session(), refs(trainees))
        }));

        let found = CheckpointStore::new(&path).load().expect("reads");
        let sent_ids = sent.borrow().clone();

        let _ = std::fs::remove_dir_all(&dir);

        (sent_ids, found)
    }

    /// The crash instant a repeated id does its damage in.
    ///
    /// Without the dedupe the repeat is still queued when the first copy is sent,
    /// so a crash mid-submit leaves it in `pending` — and `never_attempted`,
    /// which is exactly what a resume turns into work, would offer it back. The
    /// trainee is then submitted a second time against a portal that already has
    /// the first one.
    #[test]
    fn a_repeated_id_left_by_a_crash_is_not_queued_for_a_resume() {
        let (sent, found) = probe_one(None, Some(1), &["1", "1", "2"]);

        assert_eq!(sent, vec!["1"], "the repeat was never handed to the worker");

        let found = found.expect("the crash left a file");

        assert_eq!(found.total, 2, "the file accounts for two trainees, not three");
        assert_eq!(found.unaccounted(), 0, "the partition still adds up");
        assert!(
            !found.never_attempted().contains(&"1".to_string()),
            "the repeat must not survive into a resume's roster: {:?}",
            found.never_attempted()
        );
    }

    /// For every crash point the engine can be killed at, the file it leaves must
    /// keep the Gate 3 property: no trainee the worker was ever handed may appear
    /// in a resume's roster, and the three groups must still sum to `total`.
    #[test]
    fn probe_crash_matrix() {
        use crate::recovery::{self, Plan};

        let trainees = ["1", "2", "3"];

        for die_write in (0..=12)
            .flat_map(|n| [(n, false), (n, true)])
            .map(Some)
            .chain(std::iter::once(None))
        {
            for die_submit in [None, Some(1), Some(2), Some(3)] {
                let (sent, found) = probe_one(die_write, die_submit, &trainees);
                let label = format!("write={die_write:?} submit={die_submit:?}");
                let Some(cp) = found else {
                    assert!(
                        sent.is_empty(),
                        "no file but a submission was sent: {label} sent={sent:?}"
                    );
                    continue;
                };

                let accounted =
                    cp.results.len() + cp.pending.len() + usize::from(cp.in_flight.is_some());
                assert_eq!(accounted, cp.total, "partition broke: {label} {cp:?}");

                let dir = std::env::temp_dir().join(format!("dssp-probe2-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&dir).expect("scratch dir");
                let path = dir.join("dssp.checkpoint.json");
                std::fs::write(&path, serde_json::to_string(&cp).unwrap()).expect("write");
                let store = crate::store::CheckpointStore::new(&path);

                let start = recovery::guard(&store, true).expect("resume");
                let (roster, carried) = match start.plan {
                    Plan::Resume { roster, carried, .. } => (roster, carried),
                    other => panic!("expected a resume: {label} {other:?}"),
                };
                let roster_ids: Vec<String> =
                    roster.iter().filter_map(|r| r.id.clone()).collect();
                let carried_ids: Vec<String> =
                    carried.iter().map(|r| r.trainee_id.clone()).collect();

                assert_eq!(
                    roster_ids.len() + carried_ids.len(),
                    cp.total,
                    "resume arithmetic lost a trainee: {label} roster={roster_ids:?} \
                     carried={carried_ids:?} cp={cp:?}"
                );

                for id in &sent {
                    assert!(
                        !roster_ids.contains(id),
                        "resume would resubmit {id}, which was already sent: {label} \
                         sent={sent:?} roster={roster_ids:?} cp={cp:?}"
                    );
                    assert!(
                        carried_ids.contains(id),
                        "a sent trainee vanished from the carried set: {label} sent={sent:?} \
                         roster={roster_ids:?} carried={carried_ids:?} cp={cp:?}"
                    );
                }

                for id in &roster_ids {
                    assert!(
                        !carried_ids.contains(id),
                        "a trainee is in both groups: {label} {id}"
                    );
                }

                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    /// The same property across generations: crash, resume, crash again, resume.
    /// The second generation's file must still carry every trainee either run was
    /// handed, and the third resume must offer none of them.
    #[test]
    fn probe_resume_chain() {
        use crate::recovery::{self, Plan};
        use crate::store::CheckpointStore;

        let dir = std::env::temp_dir().join(format!("dssp-chain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("dssp.checkpoint.json");
        let store = CheckpointStore::new(&path);

        let mut all_sent: Vec<String> = Vec::new();
        // (generation, die_write, die_submit)
        let generations = [
            (Some((2, true)), None),
            (Some((2, true)), Some(1)),
            (None, Some(1)),
        ];

        let mut roster: Vec<TraineeRef> = refs(&["1", "2", "3"]);
        let mut carried: Vec<crate::report::TrainingResult> = Vec::new();

        for (generation, (die_write, die_submit)) in generations.into_iter().enumerate() {
            let sent = Rc::new(RefCell::new(Vec::new()));
            let mut transport = Probe {
                sent: Rc::clone(&sent),
                die_at: die_submit,
                calls: 0,
            };
            let (die_before, die_after) = match die_write {
                Some((n, false)) => (Some(n), None),
                Some((n, true)) => (None, Some(n)),
                None => (None, None),
            };
            let sink = CrashSink {
                store: CheckpointStore::new(&path),
                writes: Rc::new(Cell::new(0)),
                die_before,
                die_after,
            };

            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                BatchEngine::with_policy(fast_policy(), Box::new(sink))
                    .with_carried(carried.clone())
                    .run(&mut transport, &session(), roster.clone())
            }));

            let sent_now = sent.borrow().clone();
            all_sent.extend(sent_now.clone());

            let label = format!("generation {generation} die_write={die_write:?} die_submit={die_submit:?}");
            let found = store.load().expect("reads").expect("a file");
            let accounted =
                found.results.len() + found.pending.len() + usize::from(found.in_flight.is_some());
            assert_eq!(accounted, found.total, "partition broke: {label} {found:?}");

            let start = recovery::guard(&store, true).expect("resume");
            let (next_roster, next_carried) = match start.plan {
                Plan::Resume { roster, carried, .. } => (roster, carried),
                other => panic!("expected a resume: {label} {other:?}"),
            };
            let roster_ids: Vec<String> =
                next_roster.iter().filter_map(|r| r.id.clone()).collect();
            let carried_ids: Vec<String> =
                next_carried.iter().map(|r| r.trainee_id.clone()).collect();

            assert_eq!(
                roster_ids.len() + carried_ids.len(),
                found.total,
                "resume arithmetic lost a trainee: {label} roster={roster_ids:?} \
                 carried={carried_ids:?} file={found:?}"
            );

            for id in &all_sent {
                assert!(
                    !roster_ids.contains(id),
                    "resume would resubmit {id} after {label}: sent={all_sent:?} \
                     roster={roster_ids:?} file={found:?}"
                );
                assert!(
                    carried_ids.contains(id),
                    "{id} vanished from the carried set after {label}: sent={all_sent:?} \
                     carried={carried_ids:?} file={found:?}"
                );
            }

            for id in &roster_ids {
                assert!(
                    !carried_ids.contains(id),
                    "{id} is in both groups after {label}: {found:?}"
                );
            }

            roster = next_roster;
            carried = next_carried;
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
