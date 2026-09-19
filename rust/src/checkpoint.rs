//! Durable batch progress. Ports BatchCheckpoint.ts.
//!
//! The engine keeps its queue and results in memory. A checkpoint mirrors them
//! to disk after every trainee, because the question it answers — "what did
//! this batch already write to the portal?" — has to outlive the process.
//!
//! Serialized form stays snake_case, matching report.rs. The TS original uses
//! camelCase, so anything reading this from the extension side needs a mapping.

use serde::{Deserialize, Serialize};

use crate::report::TrainingResult;

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
    /// These are the two groups that need an operator to look at the portal
    /// directly: `indeterminate` records may have been written without a
    /// readable confirmation, and `unprocessed` ones never got as far as an
    /// attempt. Recovery reports them rather than retrying them.
    ///
    /// Note what is deliberately absent: `skipped` results are neither
    /// indeterminate nor unprocessed, because a skipped trainee was never sent
    /// and so is safe to queue again.
    pub fn unreconciled(&self) -> Unreconciled {
        Unreconciled {
            indeterminate: self
                .results
                .iter()
                .filter(|result| result.outcome == crate::report::Outcome::Indeterminate)
                .cloned()
                .collect(),
            unprocessed: self.pending.clone(),
        }
    }
}

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

    // -- persistence contract ----------------------------------------------

    /// The checkpoint is written to disk and read back by a later process, so
    /// the round trip has to preserve every field.
    #[test]
    fn a_checkpoint_round_trips_through_json() {
        let original = checkpoint(CheckpointStatus::Paused);
        let json = serde_json::to_string(&original).expect("serializes");
        let restored: BatchCheckpoint = serde_json::from_str(&json).expect("deserializes");

        assert_eq!(restored.status, CheckpointStatus::Paused);
        assert_eq!(restored.started_at, original.started_at);
        assert_eq!(restored.updated_at, original.updated_at);
        assert_eq!(restored.total, original.total);
        assert_eq!(restored.pending, original.pending);
        assert_eq!(restored.results.len(), original.results.len());
        assert_eq!(restored.results[1].outcome, Outcome::Indeterminate);
    }

    /// Lowercase on the wire, because a persisted status is read by tools that
    /// never see this enum.
    #[test]
    fn status_serializes_lowercase() {
        let json = serde_json::to_string(&CheckpointStatus::Interrupted).expect("serializes");
        assert_eq!(json, "\"interrupted\"");
    }
}
