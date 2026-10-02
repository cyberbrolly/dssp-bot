//! Durable batch progress. Ports BatchCheckpoint.ts.
//!
//! The engine keeps its queue and results in memory; a checkpoint mirrors them
//! to disk after every trainee.
//!
//! Serialized form stays snake_case, matching report.rs. The TS original uses
//! camelCase, so anything reading this from the extension side needs a mapping.

use serde::{Deserialize, Serialize};

use crate::report::{Outcome, TrainingResult};

/// Lifecycle of a persisted batch.
///
/// `Running` and `Paused` are live states: finding either one on startup means
/// the process that wrote it never reached a terminal state, so it was killed.
/// `Interrupted` records that conclusion.
///
/// `Finished` and `Aborted` are both terminal, and the difference is the point:
/// `Finished` is a batch that worked through its queue, `Aborted` one the engine
/// stopped early because a result could not be accounted for. They are kept apart
/// because the start gate has to tell them apart — a batch that closed out may be
/// run again over its slot, while one that stopped early left settled records on
/// the portal that a plain re-run would ask for a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointStatus {
    Running,
    Paused,
    Finished,
    Interrupted,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCheckpoint {
    pub status: CheckpointStatus,
    /// Matches the report's `started_at` for the same batch.
    pub started_at: String,
    /// Refreshed on every write.
    pub updated_at: String,
    /// Trainees in the batch as originally queued.
    pub total: usize,
    /// Results recorded so far, in completion order.
    pub results: Vec<TrainingResult>,
    /// Trainee ids still queued, in order. Empty once the queue drains.
    pub pending: Vec<String>,
    /// The trainee handed to the worker whose result has not come back yet.
    ///
    /// The one window in which a trainee belongs to no other field: it has left
    /// `pending` and has no entry in `results`, so a crash there would leave a
    /// possibly-submitted record in no group at all. Non-`None` only in the write
    /// taken immediately before the submission is sent.
    ///
    /// `default`, so a file written before this field existed still parses. Always
    /// serialized — `null` when idle — rather than skipped, so a reader can tell
    /// "nothing in flight" from "an older build wrote this".
    #[serde(default)]
    pub in_flight: Option<String>,
}

/// Receives each checkpoint the engine produces.
///
/// Injected rather than owned so the engine stays free of storage concerns and
/// remains testable without a filesystem — the unit tests pass a recorder, the
/// CLI passes [`crate::store::CheckpointStore`].
///
/// Implementations own their own error reporting. The engine drops the error on
/// purpose: a storage fault must not abort a batch that is otherwise submitting
/// successfully, so the sink is the only place a failed write becomes visible.
pub trait CheckpointWriter {
    fn write(&mut self, checkpoint: &BatchCheckpoint) -> Result<(), String>;
}

impl BatchCheckpoint {
    /// True while the batch that wrote this checkpoint was still expected to
    /// run.
    pub fn is_live(&self) -> bool {
        matches!(self.status, CheckpointStatus::Running | CheckpointStatus::Paused)
    }

    /// Re-mark a checkpoint abandoned by a terminated process.
    ///
    /// Returns whether the status actually changed, so a caller can skip a
    /// redundant write.
    pub fn mark_interrupted(&mut self) -> bool {
        if !self.is_live() {
            return false;
        }

        self.status = CheckpointStatus::Interrupted;
        true
    }

    /// Split out the trainees this checkpoint cannot account for.
    ///
    /// These are the groups that need an operator to look at the portal
    /// directly: `indeterminate` records may have been written without a
    /// readable confirmation, and `unprocessed` ones never got as far as an
    /// attempt. Recovery reports them rather than retrying them.
    ///
    /// Note what is deliberately absent: `skipped` results are neither
    /// indeterminate nor unprocessed, because a skipped trainee was never sent
    /// and so is safe to queue again — [`Self::never_attempted`] is where they
    /// are collected.
    pub fn unreconciled(&self) -> Unreconciled {
        let mut indeterminate: Vec<TrainingResult> = self
            .results
            .iter()
            .filter(|result| result.outcome == Outcome::Indeterminate)
            .cloned()
            .collect();

        // An in-flight trainee is in neither of the two groups on its own: it
        // was dequeued, so it is not pending, and its result never landed, so it
        // is not in `results`. It is reported with the unconfirmed rather than
        // with the work still to do, because its submission may already have
        // reached the portal — and that is the one thing recovery must never
        // queue again.
        if let Some(record) = self.in_flight_record() {
            indeterminate.push(record);
        }

        Unreconciled {
            indeterminate,
            unprocessed: self.pending.clone(),
        }
    }

    /// The unconfirmed record an in-flight trainee stands for, if any.
    ///
    /// Synthesized rather than written into `results` by the writer: the engine
    /// is dead when this window is open, and a file that invented a result would
    /// stop being a record of what was actually observed. One definition, shared
    /// by [`Self::unreconciled`] and by the carry-forward a resume performs,
    /// so the two can never disagree about what the crash left behind.
    pub fn in_flight_record(&self) -> Option<TrainingResult> {
        self.in_flight.as_ref().map(|id| TrainingResult {
            trainee_id: id.clone(),
            trainee_name: String::new(),
            outcome: Outcome::Indeterminate,
            attempts: 0,
            // A Rust-side label, not a worker error code: it distinguishes this
            // record from a submission the worker actually reported on.
            error_code: Some(IN_FLIGHT_CODE.to_string()),
            message: Some(IN_FLIGHT_MESSAGE.to_string()),
        })
    }

    /// The trainee a file from a build without in-flight tracking could have
    /// handed to the worker without recording it, if any.
    ///
    /// The head of `pending`, and only that one: such a build wrote at settle
    /// points, so its last write named the trainee it was about to work on, and
    /// everything queued behind it was still evidence of never-sent. It is the
    /// same crash window [`Self::in_flight`] names, recorded by a build that could
    /// not name it — hence recovery treats the two the same: carried, never
    /// replayed.
    ///
    /// `tracks_in_flight` comes from the file rather than from this snapshot — see
    /// `StoredCheckpoint` — because `#[serde(default)]` erases the difference
    /// between a file that had no such key and one that wrote `null`.
    pub fn untracked_suspect(&self, tracks_in_flight: bool) -> Option<TrainingResult> {
        if tracks_in_flight {
            return None;
        }

        self.pending.first().map(|id| TrainingResult {
            trainee_id: id.clone(),
            trainee_name: String::new(),
            outcome: Outcome::Indeterminate,
            attempts: 0,
            error_code: Some(LEGACY_PENDING_CODE.to_string()),
            message: Some(LEGACY_PENDING_MESSAGE.to_string()),
        })
    }

    /// The trainees a batch abort drained without attempting, in drain order.
    ///
    /// The one form of never-sent evidence nothing can hide behind: a drained
    /// row is written *after* this batch stopped working the queue, so no
    /// handoff happened behind it. That is what makes it safe to roster even for
    /// a file whose queue is not — see [`Self::unvouched_queue`].
    pub fn drained_skips(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result.outcome == Outcome::Skipped)
            .map(|result| result.trainee_id.clone())
            .collect()
    }

    /// Every trainee that was never sent, in the order it was queued.
    ///
    /// Two groups qualify and neither can produce a duplicate: the `skipped`
    /// results a batch abort drained without attempting, and the `pending` ones
    /// it never reached. Recovery may safely queue all of them again — which is
    /// what makes the spec's "pending jobs continue" reachable without an
    /// acknowledgement.
    ///
    /// Only true of a file that can vouch for its queue. For one that cannot,
    /// [`Self::drained_skips`] is the whole of the answer: see
    /// [`Self::unvouched_queue`].
    pub fn never_attempted(&self) -> Vec<String> {
        // Drain order then queue order: a crash during the drain leaves the
        // already-drained trainees in `results` and the untouched remainder in
        // `pending`, so this concatenation restores the original batch order.
        self.drained_skips()
            .into_iter()
            .chain(self.pending.iter().cloned())
            .collect()
    }

    /// Every trainee left behind the head of a pre-Stage-28 file's queue, written
    /// down as unconfirmed.
    ///
    /// [`Self::untracked_suspect`] takes the head of `pending` on the premise that
    /// such a build wrote at settle points, so its last write named the trainee it
    /// was about to work on. That holds only if the last write *landed*, and those
    /// builds dropped a failed write rather than stopping — so the file can be
    /// behind by more than one handoff, every remaining entry may already have
    /// reached the worker, and the queue is not evidence of never-sent. It must not
    /// be offered as work.
    ///
    /// Called on a snapshot whose suspect has already been promoted, so this is the
    /// tail; counting the head here too would put one trainee in two groups.
    pub fn unvouched_queue(&self) -> Vec<TrainingResult> {
        self.pending
            .iter()
            .map(|id| TrainingResult {
                trainee_id: id.clone(),
                trainee_name: String::new(),
                outcome: Outcome::Indeterminate,
                attempts: 0,
                error_code: Some(LEGACY_QUEUE_CODE.to_string()),
                message: Some(LEGACY_QUEUE_MESSAGE.to_string()),
            })
            .collect()
    }

    /// Results that reached a conclusion: `Success` or `Failed`, the two outcomes
    /// that mean the portal was asked and answered.
    ///
    /// Not `results.len()`: a `Skipped` row was drained after an abort and never
    /// sent, so counting it would let a file that submitted nothing pass for one
    /// that landed work — see [`Self::blocks_start`], which this decides.
    ///
    /// Also not `results.len() - unconfirmed`: the in-flight trainee is synthesized
    /// into the unconfirmed group without being in `results`, so subtracting it
    /// would under-report what settled.
    pub fn settled(&self) -> usize {
        self.results
            .iter()
            .filter(|result| matches!(result.outcome, Outcome::Success | Outcome::Failed))
            .count()
    }

    /// Whether this file still records work that reached the portal, on a batch
    /// that never recorded its own finish.
    ///
    /// The one clause of [`Self::blocks_start`] that reads the status — see there
    /// for why the exemption is `Finished` rather than "not live".
    ///
    /// `Aborted` counts, and that is why it is a status rather than a `Finished`
    /// file with a flag: an aborted batch stops early *after* landing whatever it
    /// landed, so it is precisely a batch that records work and did not finish.
    pub fn records_landed_work(&self) -> bool {
        self.status != CheckpointStatus::Finished && self.settled() > 0
    }

    /// Whether a new batch may take this checkpoint's slot without an operator
    /// first acknowledging what the previous one left behind.
    ///
    /// Three things can be missing from a file, and liveness settles only the last:
    ///
    /// - an unconfirmed record, including a trainee left in flight —
    ///   [`Self::has_unconfirmed`];
    /// - counts that do not add up — [`Self::unaccounted`];
    /// - **a batch that died with settled work on the file.** It still records
    ///   submissions that reached the portal, and the engine rewrites the file in
    ///   full, so a fresh start re-queues the job from the top and asks for those
    ///   trainees a second time. What would have to catch that is the portal's own
    ///   `duplicate|already logged` match — the second layer, which the runbook
    ///   lists as an assumption that can fail in the unsafe direction, not the layer
    ///   this gate is allowed to rest on.
    ///
    /// That third one is the only reason status is read here, and it is *not* the
    /// check Stage 28 removed. That one blocked on liveness alone, so it fired on
    /// every ordinary kill-and-retry where nothing was at stake — and a gate an
    /// operator learns to answer with `DSSP_RESUME=1` stops being read. This one
    /// fires only where answering it changes what happens: a killed batch that had
    /// already landed something. A kill before the first settle still starts clean,
    /// and a batch that recorded its own finish is exempt, so re-running a completed
    /// job file keeps the meaning the runbook gives it.
    ///
    /// `Finished` therefore has to mean *closed out*, not merely *terminal* — which
    /// is why an abort writes [`CheckpointStatus::Aborted`] instead. An aborted batch
    /// is the case this clause exists for, and the common one: the engine stops on a
    /// result it cannot account for, of which an expired session mid-batch is the
    /// ordinary instance, and re-running the job file after signing in again is the
    /// first thing an operator tries. Writing that as `Finished` would exempt exactly
    /// the file the gate is for.
    ///
    /// The exemption is `Finished` and not "not live" on purpose. `mark_interrupted`
    /// rewrites `Running` to `Interrupted`, so keying on liveness would make the
    /// refusal expire the moment it was recorded and let the operator through by
    /// running the command twice. The evidence has to be as durable as the `in_flight`
    /// one it sits beside — and that is how the in-flight case already behaves: an
    /// `Indeterminate` row keeps refusing every start until it is acknowledged.
    ///
    /// Never-attempted trainees still do not block on their own: nothing was sent
    /// for them, so leaving them out cannot create a duplicate. They block here only
    /// when they share a file with work that did land.
    pub fn blocks_start(&self) -> bool {
        self.has_unconfirmed() || self.unaccounted() > 0 || self.records_landed_work()
    }

    /// How many trainees this file does not place in any group.
    ///
    /// Every trainee in a batch is in exactly one of `results`, `pending` and
    /// [`Self::in_flight`], so those three always add up to [`Self::total`]. A
    /// file where they do not has lost one somewhere — and, unlike the crash
    /// window, cannot say which. That is what makes the count worth checking:
    /// it is the only part of the loss the file itself can still testify to.
    ///
    /// `saturating_sub` because a file that over-counts is exactly as
    /// untrustworthy as one that under-counts, and a negative is not a number of
    /// trainees.
    pub fn unaccounted(&self) -> usize {
        self.total.saturating_sub(
            self.results.len() + self.pending.len() + usize::from(self.in_flight.is_some()),
        )
    }

    /// Write an untracked build's suspicion down in this build's own vocabulary.
    ///
    /// A file from a build without [`Self::in_flight`] states what it knows by
    /// *omission* — the key's absence **is** the evidence — and every write this
    /// build makes adds that key, so the first rewrite would destroy the very thing
    /// [`Self::untracked_suspect`] reads: the next start would take a silent file
    /// for a clean one and submit a trainee the dead build may already have sent.
    /// Record the suspicion *before* any rewrite of the file.
    ///
    /// Moving the trainee from `pending` to `in_flight` keeps the partition and
    /// takes it out of [`Self::never_attempted`], which stops a resume from running
    /// it.
    ///
    /// Returns whether it changed anything, so a caller can skip a redundant write.
    pub fn promote_suspect(&mut self, id: &str) -> bool {
        // A guard, not the protection. The caller cannot get here: `guard`
        // promotes only what `untracked_suspect` hands it, and that is `None` for
        // any file carrying the `in_flight` key — the same key whose presence is
        // what makes this `Some`. What stops a suspect being re-run is the
        // promotion itself, which takes it out of `never_attempted`, plus the
        // `suspect_id` filter on the resume roster. Should a future caller ever
        // be able to reach this, the rule stands: a real in-flight record is
        // direct evidence and a suspicion inferred from a queue is not.
        if self.in_flight.is_some() {
            return false;
        }

        let Some(at) = self.pending.iter().position(|queued| queued == id) else {
            return false;
        };

        self.pending.remove(at);
        self.in_flight = Some(id.to_string());

        true
    }

    /// Whether any record here may already exist on the portal without a
    /// readable confirmation.
    fn has_unconfirmed(&self) -> bool {
        self.in_flight.is_some()
            || self
                .results
                .iter()
                .any(|result| result.outcome == Outcome::Indeterminate)
    }
}

/// Error code on the synthesized record for a trainee left in flight by a crash.
pub const IN_FLIGHT_CODE: &str = "CRASH_IN_FLIGHT";
/// Why that record is unconfirmed, in the operator's terms.
pub const IN_FLIGHT_MESSAGE: &str =
    "dequeued but no result was recorded — the submission may have reached the portal";

/// Error code on the synthesized record for the trainee a pre-Stage-28 file left
/// at the head of its queue. Distinct from [`IN_FLIGHT_CODE`]: one is a file that
/// named the trainee it was submitting, the other a file that could not.
pub const LEGACY_PENDING_CODE: &str = "CRASH_UNTRACKED_PENDING";

/// Why that record is unconfirmed, in the operator's terms.
pub const LEGACY_PENDING_MESSAGE: &str =
    "was next in queue when a build without in-flight tracking died — the submission may have \
     reached the portal";

/// Error code on the synthesized record for a trainee left *behind* the head of a
/// pre-Stage-28 file's queue.
///
/// Distinct from [`LEGACY_PENDING_CODE`] because the two make different claims:
/// the head is the one the dead build was most likely working on, the tail one it
/// may equally well have reached. One message for both would have to be vague
/// about the one thing the operator acts on.
pub const LEGACY_QUEUE_CODE: &str = "CRASH_UNTRACKED_QUEUE";

/// Why that record is unconfirmed, in the operator's terms.
pub const LEGACY_QUEUE_MESSAGE: &str =
    "was still queued in a file that cannot say how far it had got — the submission may have \
     reached the portal";

#[derive(Debug, Clone, Serialize)]
pub struct Unreconciled {
    pub indeterminate: Vec<TrainingResult>,
    pub unprocessed: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Outcome;

    fn result(trainee_id: &str, outcome: Outcome) -> TrainingResult {
        TrainingResult {
            trainee_id: trainee_id.to_string(),
            trainee_name: format!("Trainee {trainee_id}"),
            outcome,
            attempts: 1,
            error_code: None,
            message: None,
        }
    }

    fn checkpoint(status: CheckpointStatus) -> BatchCheckpoint {
        BatchCheckpoint {
            status,
            started_at: "2026-09-19T10:00:00Z".to_string(),
            updated_at: "2026-09-19T10:05:00Z".to_string(),
            total: 4,
            results: vec![
                result("1", Outcome::Success),
                result("2", Outcome::Indeterminate),
            ],
            pending: vec!["3".to_string(), "4".to_string()],
            in_flight: None,
        }
    }

    #[test]
    fn running_and_paused_are_live() {
        assert!(checkpoint(CheckpointStatus::Running).is_live());
        assert!(checkpoint(CheckpointStatus::Paused).is_live());
    }

    #[test]
    fn terminal_states_are_not_live() {
        assert!(!checkpoint(CheckpointStatus::Finished).is_live());
        assert!(!checkpoint(CheckpointStatus::Interrupted).is_live());
    }

    /// A live checkpoint is what a killed process leaves behind, so this is the
    /// case recovery exists for.
    #[test]
    fn marking_a_live_checkpoint_interrupted_reports_a_change() {
        let mut live = checkpoint(CheckpointStatus::Running);
        assert!(live.mark_interrupted());
        assert_eq!(live.status, CheckpointStatus::Interrupted);
    }

    #[test]
    fn marking_a_paused_checkpoint_interrupted_reports_a_change() {
        let mut paused = checkpoint(CheckpointStatus::Paused);
        assert!(paused.mark_interrupted());
        assert_eq!(paused.status, CheckpointStatus::Interrupted);
    }

    /// Already-terminal means the process exited cleanly; re-marking it must
    /// report no change so the caller can skip a redundant write.
    #[test]
    fn marking_a_finished_checkpoint_is_a_no_op() {
        let mut finished = checkpoint(CheckpointStatus::Finished);
        assert!(!finished.mark_interrupted());
        assert_eq!(finished.status, CheckpointStatus::Finished);
    }

    #[test]
    fn marking_an_already_interrupted_checkpoint_is_a_no_op() {
        let mut interrupted = checkpoint(CheckpointStatus::Interrupted);
        assert!(!interrupted.mark_interrupted());
        assert_eq!(interrupted.status, CheckpointStatus::Interrupted);
    }

    #[test]
    fn marking_interrupted_preserves_the_progress() {
        let mut live = checkpoint(CheckpointStatus::Running);
        live.mark_interrupted();
        assert_eq!(live.results.len(), 2);
        assert_eq!(live.pending, vec!["3".to_string(), "4".to_string()]);
        assert_eq!(live.total, 4);
    }

    #[test]
    fn only_indeterminate_results_are_unreconciled() {
        let split = checkpoint(CheckpointStatus::Running).unreconciled();
        assert_eq!(split.indeterminate.len(), 1);
        assert_eq!(split.indeterminate[0].trainee_id, "2");
    }

    /// A skipped trainee was never sent, so recovery may safely queue it again
    /// — it must not be reported as needing a human.
    #[test]
    fn skipped_results_are_not_unreconciled() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.results = vec![
            result("1", Outcome::Success),
            result("2", Outcome::Skipped),
            result("3", Outcome::Failed),
        ];
        cp.pending = Vec::new();

        let split = cp.unreconciled();
        assert!(split.indeterminate.is_empty(), "{:?}", split.indeterminate);
        assert!(split.unprocessed.is_empty(), "{:?}", split.unprocessed);
    }

    #[test]
    fn pending_entries_are_reported_unprocessed_in_order() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.pending = vec!["7".to_string(), "8".to_string(), "9".to_string()];
        assert_eq!(cp.unreconciled().unprocessed, vec!["7", "8", "9"]);
    }

    #[test]
    fn a_clean_finished_batch_unreconciles_to_nothing() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.results = vec![result("1", Outcome::Success), result("2", Outcome::Success)];
        cp.pending = Vec::new();

        let split = cp.unreconciled();
        assert!(split.indeterminate.is_empty());
        assert!(split.unprocessed.is_empty());
    }

    #[test]
    fn both_groups_are_reported_together() {
        let split = checkpoint(CheckpointStatus::Interrupted).unreconciled();
        assert_eq!(split.indeterminate.len(), 1);
        assert_eq!(split.unprocessed.len(), 2);
    }

    /// The shape a process killed mid-submission leaves behind: trainee 2 was
    /// dequeued and handed to the worker, so it is in neither `results` nor
    /// `pending`.
    fn interrupted_mid_submission() -> BatchCheckpoint {
        BatchCheckpoint {
            status: CheckpointStatus::Running,
            started_at: "2026-09-19T10:00:00Z".to_string(),
            updated_at: "2026-09-19T10:05:00Z".to_string(),
            total: 4,
            results: vec![result("1", Outcome::Success)],
            pending: vec!["3".to_string(), "4".to_string()],
            in_flight: Some("2".to_string()),
        }
    }

    /// The gap this whole field closes: without it the trainee named here is in
    /// no group at all, and recovery would requeue a submission that may have
    /// reached the portal.
    #[test]
    fn an_in_flight_trainee_is_reported_with_the_unconfirmed() {
        let split = interrupted_mid_submission().unreconciled();

        assert_eq!(split.indeterminate.len(), 1);
        assert_eq!(split.indeterminate[0].trainee_id, "2");
        assert_eq!(split.indeterminate[0].outcome, Outcome::Indeterminate);
        assert_eq!(
            split.indeterminate[0].error_code.as_deref(),
            Some(IN_FLIGHT_CODE)
        );
        assert_eq!(split.unprocessed, vec!["3", "4"]);
    }

    /// Being unconfirmed is what keeps it out of the requeueable set: an
    /// in-flight trainee must never be offered to the queue again.
    #[test]
    fn an_in_flight_trainee_is_not_never_attempted() {
        let cp = interrupted_mid_submission();

        assert_eq!(cp.never_attempted(), vec!["3", "4"]);
        assert!(!cp.unreconciled().unprocessed.contains(&"2".to_string()));
    }

    /// Every write the engine takes leaves each trainee in exactly one of the
    /// three groups — the property the gate and the recovery split both rest on.
    #[test]
    fn every_trainee_sits_in_exactly_one_group() {
        for cp in [
            checkpoint(CheckpointStatus::Running),
            interrupted_mid_submission(),
            checkpoint(CheckpointStatus::Finished),
        ] {
            let accounted = cp.results.len() + cp.pending.len() + usize::from(cp.in_flight.is_some());

            assert_eq!(accounted, cp.total, "{cp:?}");
        }
    }

    /// A batch abort drains the rest of the queue as skipped without ever
    /// sending it, so those ids — not just `pending` — are what a resume may
    /// safely run again.
    #[test]
    fn never_attempted_collects_the_drained_skips_too() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.results = vec![
            result("1", Outcome::Success),
            result("2", Outcome::Skipped),
            result("3", Outcome::Skipped),
        ];
        cp.pending = Vec::new();

        assert_eq!(cp.never_attempted(), vec!["2", "3"]);
    }

    #[test]
    fn never_attempted_restores_queue_order_across_a_partial_drain() {
        let mut cp = checkpoint(CheckpointStatus::Interrupted);
        cp.results = vec![result("1", Outcome::Success), result("2", Outcome::Skipped)];
        cp.pending = vec!["3".to_string(), "4".to_string()];

        assert_eq!(cp.never_attempted(), vec!["2", "3", "4"]);
    }

    #[test]
    fn a_settled_trainee_is_not_never_attempted() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.results = vec![
            result("1", Outcome::Success),
            result("2", Outcome::Failed),
            result("3", Outcome::Indeterminate),
        ];
        cp.pending = Vec::new();

        assert!(cp.never_attempted().is_empty(), "{:?}", cp.never_attempted());
    }

    /// A live checkpoint is a killed process; its last trainee's outcome may be
    /// unknown, so the next batch must not overwrite the only account of it.
    #[test]
    fn a_live_checkpoint_blocks_a_new_start() {
        assert!(checkpoint(CheckpointStatus::Running).blocks_start());
        assert!(checkpoint(CheckpointStatus::Paused).blocks_start());
    }

    /// Terminal status is not enough on its own: a batch that aborted cleanly
    /// still leaves a record that may already exist on the portal.
    #[test]
    fn an_unconfirmed_record_blocks_a_start_even_from_a_finished_batch() {
        assert!(checkpoint(CheckpointStatus::Finished).blocks_start());
    }

    #[test]
    fn an_in_flight_trainee_blocks_a_start() {
        assert!(interrupted_mid_submission().blocks_start());
    }

    /// A reconciled batch whose records all landed owes nothing, so its slot may
    /// be reused without ceremony.
    #[test]
    fn a_finished_batch_of_settled_records_does_not_block_a_start() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.total = 3;
        cp.results = vec![
            result("1", Outcome::Success),
            result("2", Outcome::Failed),
            result("3", Outcome::Skipped),
        ];
        cp.pending = Vec::new();

        assert!(!cp.blocks_start());
    }

    /// Nothing was sent for a never-attempted trainee, so leaving it out of a
    /// new batch cannot create a duplicate — it must not require an override.
    ///
    /// The fixture carries no settled row on purpose. One would block on
    /// [`Self::records_landed_work`], and this test would stop isolating the claim
    /// it is named for — which is what it did before that clause existed. The
    /// contrast is pinned by `a_landed_row_blocks_even_alongside_never_attempted_ones`.
    #[test]
    fn never_attempted_trainees_alone_do_not_block_a_start() {
        let mut cp = checkpoint(CheckpointStatus::Interrupted);
        cp.total = 3;
        cp.results = vec![result("1", Outcome::Skipped)];
        cp.pending = vec!["2".to_string(), "3".to_string()];

        assert_eq!(cp.settled(), 0, "nothing in this file landed");
        assert_eq!(cp.unaccounted(), 0);
        assert!(!cp.blocks_start());
    }

    /// The other half of the rule above, and the hole Stage 28 left open: those
    /// trainees are harmless *alone*. Sharing a file with work that did land makes
    /// a plain start re-queue the job from the top and ask the portal for the landed
    /// trainees a second time — which is what the portal's `duplicate|already logged`
    /// match would have to catch, and it is the second layer, not this gate.
    #[test]
    fn a_landed_row_blocks_even_alongside_never_attempted_ones() {
        let mut cp = checkpoint(CheckpointStatus::Interrupted);
        cp.total = 3;
        cp.results = vec![result("1", Outcome::Success), result("2", Outcome::Skipped)];
        cp.pending = vec!["3".to_string()];

        assert_eq!(cp.settled(), 1, "trainee 1 reached the portal");
        assert!(cp.records_landed_work());
        assert!(cp.blocks_start());
    }

    /// Only `Success` and `Failed` count as landed. A `Skipped` row was drained
    /// after an abort and never sent, so a file of nothing but skips has landed
    /// nothing — counting them would refuse a start over work that never happened.
    #[test]
    fn drained_skips_are_not_landed_work() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.total = 2;
        cp.results = vec![result("1", Outcome::Skipped)];
        cp.pending = vec!["2".to_string()];

        assert_eq!(cp.settled(), 0);
        assert!(!cp.records_landed_work());
        assert!(!cp.blocks_start());
    }

    /// A batch that wrote its own finish is exempt, which is what keeps re-running
    /// a completed job file meaning what the runbook says it means — a re-run that
    /// submits everyone again, not a refusal. The refusal would be permanent
    /// otherwise: nothing rewrites a `Finished` file, so the landed row stays.
    #[test]
    fn a_finished_batch_is_exempt_from_the_landed_work_clause() {
        let cp = checkpoint(CheckpointStatus::Finished);

        assert!(cp.settled() > 0, "it landed work");
        assert!(!cp.records_landed_work());
    }

    /// And the refusal outlives the write that records it. Keying this on
    /// `is_live()` would let the operator straight through by running the command
    /// twice, because `mark_interrupted` rewrites `Running` to `Interrupted` —
    /// so the evidence has to survive that, exactly as `in_flight` does.
    #[test]
    fn marking_a_killed_batch_interrupted_does_not_expire_the_refusal() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.total = 1;
        cp.results = vec![result("1", Outcome::Success)];
        cp.pending = Vec::new();

        assert!(cp.blocks_start(), "refused on the first start");

        cp.mark_interrupted();

        assert!(!cp.is_live(), "the status is no longer live");
        assert!(cp.blocks_start(), "and it must still refuse the next start");
    }

    /// Every trainee is in exactly one of `results`, `pending` and `in_flight`,
    /// so anything less than `total` means one has gone missing — and a file
    /// that cannot place a trainee cannot be trusted to be complete either.
    #[test]
    fn a_file_that_does_not_account_for_every_trainee_blocks_a_start() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.results = vec![result("1", Outcome::Success), result("2", Outcome::Success)];
        cp.pending = Vec::new();
        cp.in_flight = None;

        assert_eq!(cp.unaccounted(), 2);
        assert!(cp.blocks_start(), "two of its four trainees are in no group");
    }

    /// The counterweight, so the check cannot be satisfied by blocking always.
    #[test]
    fn a_file_that_accounts_for_every_trainee_does_not_block_on_that_ground() {
        assert_eq!(checkpoint(CheckpointStatus::Finished).unaccounted(), 0);
        assert_eq!(interrupted_mid_submission().unaccounted(), 0);
    }

    /// Over-counting is as untrustworthy as under-counting, and a negative is
    /// not a number of trainees.
    #[test]
    fn a_file_that_over_counts_reports_nothing_unaccounted() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.total = 1;

        assert_eq!(cp.unaccounted(), 0);
    }

    /// The regression this exists for: recording the suspicion is a *write*, and
    /// every write adds the `in_flight` key a legacy file's evidence consists of not
    /// having. Promoting puts the fact into the file's fields first, so it survives
    /// being rewritten.
    #[test]
    fn promoting_a_suspect_moves_it_out_of_the_queue_and_into_flight() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.in_flight = None;
        let before = cp.unaccounted();

        assert!(cp.promote_suspect("3"));

        assert_eq!(cp.in_flight.as_deref(), Some("3"));
        assert_eq!(cp.pending, vec!["4".to_string()], "and it left the queue");
        assert_eq!(cp.unaccounted(), before, "the partition holds across it");
        assert!(
            !cp.never_attempted().contains(&"3".to_string()),
            "so a resume cannot run it"
        );
    }

    /// Why the guard inside [`BatchCheckpoint::promote_suspect`] cannot fire for its
    /// one caller: a file that names a trainee in flight is precisely one that tracks
    /// in flight, and such a file has no untracked suspect to promote.
    #[test]
    fn a_file_that_records_in_flight_has_no_suspect_to_promote() {
        let cp = interrupted_mid_submission();

        assert!(cp.in_flight.is_some(), "the file names a trainee in flight");
        assert!(
            cp.untracked_suspect(true).is_none(),
            "so the caller has nothing to promote, and never reaches the guard"
        );
    }

    /// The guard itself, called directly — the only way to reach it. It pins the
    /// branch against a future edit, nothing more: what keeps a suspect from being
    /// re-run is the promotion (out of [`Self::never_attempted`]) plus the
    /// `suspect_id` filter on the resume roster.
    #[test]
    fn promoting_a_suspect_never_displaces_a_real_in_flight_record() {
        let mut cp = interrupted_mid_submission();

        assert!(!cp.promote_suspect("3"));
        assert_eq!(cp.in_flight.as_deref(), Some("2"));
        assert_eq!(cp.pending, vec!["3".to_string(), "4".to_string()]);
    }

    /// Nothing to promote, nothing changes — the caller uses this to decide
    /// whether a write is needed at all.
    #[test]
    fn promoting_a_trainee_that_is_not_queued_changes_nothing() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.in_flight = None;
        let before = cp.clone();

        assert!(!cp.promote_suspect("99"));
        assert_eq!(cp.pending, before.pending);
        assert_eq!(cp.in_flight, before.in_flight);
    }

    /// Such a build wrote only at settle points, so its last write named the
    /// trainee it was about to work on. That one may already be on the portal;
    /// the rest of the queue was not yet dequeued and is still evidence.
    #[test]
    fn an_untracked_file_suspects_only_the_head_of_its_queue() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.results = vec![result("1", Outcome::Success)];
        cp.pending = vec!["2".to_string(), "3".to_string()];

        let suspect = cp.untracked_suspect(false).expect("a suspect");

        assert_eq!(suspect.trainee_id, "2");
        assert_eq!(suspect.outcome, Outcome::Indeterminate);
        assert_eq!(suspect.error_code.as_deref(), Some(LEGACY_PENDING_CODE));
    }

    #[test]
    fn a_tracked_file_has_no_untracked_suspect() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.pending = vec!["2".to_string()];

        assert!(cp.untracked_suspect(true).is_none());
    }

    #[test]
    fn an_empty_queue_leaves_nothing_to_suspect() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.pending = Vec::new();

        assert!(cp.untracked_suspect(false).is_none());
    }

    /// The checkpoint is written to disk and read back by a later process, so
    /// the round trip has to preserve every field.
    #[test]
    fn a_checkpoint_round_trips_through_json() {
        let original = interrupted_mid_submission();
        let json = serde_json::to_string(&original).expect("serializes");
        let restored: BatchCheckpoint = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(restored.status, CheckpointStatus::Running);
        assert_eq!(restored.started_at, original.started_at);
        assert_eq!(restored.updated_at, original.updated_at);
        assert_eq!(restored.total, original.total);
        assert_eq!(restored.pending, original.pending);
        assert_eq!(restored.in_flight, original.in_flight);
        assert_eq!(restored.results.len(), original.results.len());
    }

    /// A checkpoint written before `in_flight` existed is still a record of what
    /// a batch submitted, so it has to load — an unreadable file is the one
    /// thing recovery must never mistake for "no batch ran".
    #[test]
    fn a_checkpoint_without_an_in_flight_field_still_parses() {
        let text = r#"{
            "status": "interrupted",
            "started_at": "1758285600000",
            "updated_at": "1758285900000",
            "total": 2,
            "results": [
                {
                    "trainee_id": "1",
                    "trainee_name": "Trainee 1",
                    "outcome": "indeterminate",
                    "attempts": 1
                }
            ],
            "pending": ["2"]
        }"#;

        let restored: BatchCheckpoint = serde_json::from_str(text).expect("parses");

        assert_eq!(restored.in_flight, None);
        assert_eq!(restored.unreconciled().indeterminate.len(), 1);
        assert!(restored.blocks_start());
    }

    /// The field is always on the wire, even when there is nothing in flight:
    /// a reader that never saw this struct can then tell "idle" from "older
    /// build wrote this".
    #[test]
    fn an_idle_checkpoint_serializes_a_null_in_flight() {
        let json = serde_json::to_string(&checkpoint(CheckpointStatus::Running)).expect("serializes");

        assert!(json.contains("\"in_flight\":null"), "{json}");
    }

    /// Lowercase on the wire, because a persisted status is read by tools that
    /// never see this enum.
    #[test]
    fn status_serializes_lowercase() {
        let json = serde_json::to_string(&CheckpointStatus::Interrupted).expect("serializes");
        assert_eq!(json, "\"interrupted\"");
    }

    /// Every status has to survive a round trip, because the file is the only
    /// account of a run that never printed a report — and `aborted` is the one a
    /// reader must not confuse with `finished`.
    #[test]
    fn every_status_round_trips_through_the_file() {
        for status in [
            CheckpointStatus::Running,
            CheckpointStatus::Paused,
            CheckpointStatus::Finished,
            CheckpointStatus::Interrupted,
            CheckpointStatus::Aborted,
        ] {
            let text = serde_json::to_string(&status).expect("serializes");
            assert_eq!(
                serde_json::from_str::<CheckpointStatus>(&text).expect("parses"),
                status,
                "{text}"
            );
        }
    }

    /// An aborted batch is terminal, so it is not a kill to record — but it does
    /// block, which is the whole reason it is not written as `Finished`.
    #[test]
    fn an_aborted_batch_is_terminal_but_still_blocks_a_start() {
        let mut cp = checkpoint(CheckpointStatus::Aborted);

        assert!(!cp.is_live());
        assert!(!cp.mark_interrupted(), "already terminal");
        assert_eq!(cp.status, CheckpointStatus::Aborted);
        assert!(cp.settled() > 0, "it landed work before it stopped");
        assert!(cp.blocks_start());
    }
}
