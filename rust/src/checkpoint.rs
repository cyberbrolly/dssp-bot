//! Durable batch progress. Ports BatchCheckpoint.ts.
//!
//! The engine keeps its queue and results in memory. A checkpoint mirrors them
//! to disk after every trainee, because the question it answers — "what did
//! this batch already write to the portal?" — has to outlive the process.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointStatus {
    Running,
    Paused,
    Finished,
    Interrupted,
}

/// A durable snapshot of batch progress.
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
    /// `pending` and has no entry in `results`, so before this field existed a
    /// crash there left a possibly-submitted record in no group at all, and
    /// recovery had nothing to go on. Non-`None` only in the write taken
    /// immediately before the submission is sent.
    ///
    /// `default`, so a file written before this field existed still parses.
    /// Always serialized — `null` when idle — rather than skipped, so a reader
    /// can tell "nothing in flight" from "an older build wrote this", and so
    /// the on-disk field-name test stays a real pin.
    #[serde(default)]
    pub in_flight: Option<String>,
}

/// Receives each checkpoint the engine produces.
///
/// Ports `CheckpointWriter` from BatchCheckpoint.ts, which the original port
/// left out because nothing consumed it yet; the engine now wires a sink, so
/// the trait belongs here rather than in the store that happens to implement
/// it. Injected rather than owned so the engine stays free of storage concerns
/// and remains testable without a filesystem — the unit tests pass a recorder,
/// the CLI passes [`crate::store::CheckpointStore`].
///
/// Implementations own their own error reporting. The engine drops the error on
/// purpose: a storage fault must not abort a batch that is otherwise submitting
/// successfully, so the sink is the only place a failed write becomes visible.
///
/// The TS signature is `(checkpoint) => void | Promise<void>`; there is no
/// async here, so a write is an ordinary call that returns its outcome rather
/// than rejecting.
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
    /// redundant write. The TS original gets this by returning the input
    /// unchanged and letting the caller compare references — an idiom Rust
    /// does not need, so the answer is returned directly.
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
    /// The head of `pending`, and only that one. Such a build wrote at settle
    /// points, so its last write named the trainee it was about to work on —
    /// everything queued behind that one was not yet dequeued, and is therefore
    /// still evidence of never-sent. This is the same crash window [`Self::in_flight`]
    /// names, recorded by a build that had no way to name it, which is why
    /// recovery treats the two the same way: carried, never replayed.
    ///
    /// `tracks_in_flight` comes from the file rather than from this snapshot —
    /// see `StoredCheckpoint` — because `#[serde(default)]` erases the difference
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

    /// Every trainee that was never sent, in the order it was queued.
    ///
    /// Two groups qualify and neither can produce a duplicate: the `skipped`
    /// results a batch abort drained without attempting, and the `pending` ones
    /// it never reached. Recovery may safely queue all of them again — which is
    /// what makes the spec's "pending jobs continue" reachable without an
    /// acknowledgement.
    pub fn never_attempted(&self) -> Vec<String> {
        let skipped = self
            .results
            .iter()
            .filter(|result| result.outcome == Outcome::Skipped)
            .map(|result| result.trainee_id.clone());

        // Drain order then queue order: a crash during the drain leaves the
        // already-drained trainees in `results` and the untouched remainder in
        // `pending`, so this concatenation restores the original batch order.
        skipped.chain(self.pending.iter().cloned()).collect()
    }

    /// Whether a new batch may take this checkpoint's slot without an operator
    /// first acknowledging what the previous one left behind.
    ///
    /// The question is only ever "is this file missing a submission?", never
    /// "was the process killed?". Before [`Self::in_flight`] existed those were
    /// the same question — which is why the TS original blocks on a live status
    /// — but a live file whose counts all add up now says outright that nothing
    /// was in flight when it died, and refusing it would be a false alarm. A
    /// gate that cries wolf is one an operator learns to wave through with
    /// `DSSP_RESUME=1`, and that habit is what makes the real refusal worthless.
    ///
    /// Two things can still be missing, and liveness settles neither: an
    /// unconfirmed record (which includes a trainee left in flight), and counts
    /// that do not add up at all — see [`Self::unaccounted`].
    ///
    /// Never-attempted trainees do not block on their own: nothing was sent for
    /// them, so leaving them out of a new batch cannot create a duplicate.
    pub fn blocks_start(&self) -> bool {
        self.has_unconfirmed() || self.unaccounted() > 0
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
    /// build makes adds that key. So the first rewrite of a legacy file would
    /// destroy the very thing [`Self::untracked_suspect`] reads: the next start
    /// would take a silent file for a clean one and submit a trainee the dead
    /// build may already have sent. Recording the suspicion therefore has to
    /// come *before* anything that rewrites the file, or the recording is what
    /// erases it.
    ///
    /// Moving the trainee out of `pending` and into `in_flight` keeps the
    /// partition — it leaves one group as it enters the other — and takes it out
    /// of [`Self::never_attempted`], which is what stops a resume from running
    /// it.
    ///
    /// Returns whether it changed anything, so a caller can skip a redundant
    /// write.
    pub fn promote_suspect(&mut self, id: &str) -> bool {
        // A real in-flight record is better evidence than a suspicion inferred
        // from a queue, so it is never overwritten by one.
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
/// at the head of its queue. Distinct from [`IN_FLIGHT_CODE`] because the two
/// say different things to whoever reads the report: one is a file that named the
/// trainee it was submitting and the other is a file that could not.
pub const LEGACY_PENDING_CODE: &str = "CRASH_UNTRACKED_PENDING";

/// Why that record is unconfirmed, in the operator's terms.
pub const LEGACY_PENDING_MESSAGE: &str =
    "was next in queue when a build without in-flight tracking died — the submission may have \
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

    // -- liveness -----------------------------------------------------------

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

    // -- mark_interrupted ---------------------------------------------------

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

    // -- unreconciled -------------------------------------------------------

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

    // -- the crash window ---------------------------------------------------

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

    // -- never_attempted ----------------------------------------------------

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

    /// A crash during the drain leaves some in `results` and some in `pending`;
    /// drain order then queue order is the order they were originally queued in.
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

    // -- the start gate -----------------------------------------------------

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

    /// The crash-window trainee, which is why the field exists at all.
    #[test]
    fn an_in_flight_trainee_blocks_a_start() {
        assert!(interrupted_mid_submission().blocks_start());
    }

    /// A reconciled batch whose records all landed owes nothing, so its slot may
    /// be reused without ceremony.
    #[test]
    fn a_finished_batch_of_settled_records_does_not_block_a_start() {        let mut cp = checkpoint(CheckpointStatus::Finished);
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
    #[test]
    fn never_attempted_trainees_alone_do_not_block_a_start() {
        let mut cp = checkpoint(CheckpointStatus::Interrupted);
        cp.results = vec![result("1", Outcome::Success), result("2", Outcome::Skipped)];
        cp.pending = vec!["3".to_string()];

        assert!(!cp.blocks_start());
    }

    // -- files from a build without in-flight tracking ----------------------

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

    /// A file that *can* record in flight says what it knows, and it does not
    /// need the head of the queue to stand in for the marker.
    #[test]
    fn a_tracked_file_has_no_untracked_suspect() {
        let mut cp = checkpoint(CheckpointStatus::Running);
        cp.pending = vec!["2".to_string()];

        assert!(cp.untracked_suspect(true).is_none());
    }

    /// Nothing queued, nothing suspect: the file's own records account for
    /// everything it could have submitted.
    #[test]
    fn an_empty_queue_leaves_nothing_to_suspect() {
        let mut cp = checkpoint(CheckpointStatus::Finished);
        cp.pending = Vec::new();

        assert!(cp.untracked_suspect(false).is_none());
    }

    // -- persistence contract ----------------------------------------------

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
}
