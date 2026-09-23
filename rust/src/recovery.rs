//! Reconciling the checkpoint slot before a batch starts.
//!
//! Up to Stage 27 the engine only *wrote* checkpoints. This is the reading half:
//! a new batch must not take the slot from a predecessor that still owes an
//! answer, because the engine rewrites the file in full, so the predecessor's
//! records would be gone — and the loss would look like a clean run, not a
//! loss.
//!
//! The decision is a function rather than a rule inside `run_batch` so it can be
//! tested without a worker, a portal or a batch. Stage 29's extension bridge is
//! a second start path and must call [`guard`] before its own — not because it
//! would otherwise be the only way in that skips this (it would not: `run_single`
//! is one already, deliberately — it submits once, keeps no durable state, and
//! warns before it submits instead), but because any path that can *overwrite*
//! the slot has to read it first, and the daemon can.

use std::path::Path;

use crate::checkpoint::{BatchCheckpoint, Unreconciled};
use crate::protocol::TraineeRef;
use crate::report::{Outcome, TrainingResult};
use crate::store::CheckpointStore;

/// The operator's acknowledgement that they have read a predecessor's
/// unconfirmed records and accept that the new run will not touch them.
///
/// The `DSSP_*` convention, like `DSSP_CHECKPOINT` and `DSSP_WORKER_PY`. Set and
/// not one of the false spellings means on — see [`is_affirmative`].
pub const RESUME_ENV: &str = "DSSP_RESUME";

/// What a start may do with the slot, once the predecessor has been read.
#[derive(Debug)]
pub struct Start {
    pub plan: Plan,
    /// The predecessor's checkpoint as it now stands, when there was one.
    pub previous: Option<BatchCheckpoint>,
    /// Whether this start rewrote the status, i.e. found a killed process. Worth
    /// reporting even on a start that was never going to be refused: an operator
    /// who came back to run a normal batch should learn that one was.
    pub recovered: bool,
}

#[derive(Debug)]
pub enum Plan {
    /// Nothing to continue. [`BatchCheckpoint::never_attempted`] on `previous`
    /// still names trainees this run will not include unless the job file does.
    Fresh,
    /// Continue a predecessor.
    Resume {
        /// Trainees it never attempted, in queue order. Safe to run: nothing was
        /// sent for them, so running one cannot produce a duplicate.
        roster: Vec<TraineeRef>,
        /// Everything it recorded, to be carried into the new batch so its
        /// checkpoint cannot drop it. Never run again.
        carried: Vec<TrainingResult>,
        /// How many of those a human still owes an answer for: submissions that
        /// may already be on the portal. The rest of `carried` is history — the
        /// rows that landed, which are carried so the file stays whole and not
        /// because anything is outstanding.
        owed: usize,
    },
}

/// Read the slot and decide what the run may do with it.
///
/// `resume` is the operator's acknowledgement. Without it, a slot that still
/// holds an answer refuses the start; with it, the predecessor's un-attempted
/// trainees become the roster and its unconfirmed records ride along untouched.
pub fn guard(store: &CheckpointStore, resume: bool) -> Result<Start, String> {
    let Some(loaded) = store
        .load_with_provenance()
        .map_err(|e| unreadable(store.path(), &e))?
    else {
        return Ok(Start {
            plan: Plan::Fresh,
            previous: None,
            recovered: false,
        });
    };

    let mut previous = loaded.checkpoint;
    // A file from a build that could not record a submission in flight has no
    // marker to consult, so the head of its queue has to stand in for one: that
    // build wrote only at settle points, which makes the first queued trainee the
    // one it may have been submitting when it died. Everything behind it was not
    // yet dequeued, so the rest of the queue is still evidence of never-sent —
    // which is why the whole file does not have to be written off.
    let suspect = previous.untracked_suspect(loaded.tracks_in_flight);

    // Before anything reads this file's counts, and before anything rewrites it.
    // That suspicion lives in the *absence* of a key, and every write from here
    // on adds it — so recording the interruption is otherwise the act that
    // erases what the refusal is based on. An operator who then followed the
    // refusal's own advice would be told there was nothing to worry about, and
    // the trainee the dead build may already have sent would be submitted again.
    // Promoting it into `in_flight` first is what makes the suspicion outlive
    // the write, and it leaves `pending` as it enters `in_flight`, so the
    // partition still holds.
    if let Some(record) = &suspect {
        previous.promote_suspect(&record.trainee_id);
    }

    // Read off the file as the dead process left it. `mark_interrupted` is not
    // part of the question — see `blocks_start` — so the order of the two no
    // longer matters, but the promotion above does: `unreconciled` has to see
    // the suspect as in flight, or it counts it twice.
    let blocked = previous.blocks_start();
    let unreconciled = previous.unreconciled();
    let recovered = previous.mark_interrupted();

    if previous.unaccounted() > 0 {
        // Said here rather than only in the refusal, because a resume is allowed
        // to proceed over a file like this: it runs nothing outside
        // `never_attempted`, so it cannot resubmit whoever is missing, but the
        // operator still has to know the file does not add up.
        eprintln!(
            "dssp-bot: {} accounts for {} of its {} trainee(s) — {} cannot be placed and may \
             have been submitted without being recorded",
            store.path().display(),
            previous.total - previous.unaccounted(),
            previous.total,
            previous.unaccounted(),
        );
    }

    if recovered || suspect.is_some() {
        // Losing this write to a storage fault only means the next start refuses
        // again, so the failure is reported and the run continues to the
        // decision below rather than dying here — that decision is what actually
        // protects the record. On the resume path the promotion survives anyway,
        // because the engine's first write carries the record into `results`.
        if let Err(e) = store.save(&previous) {
            eprintln!("dssp-bot: could not record the interruption: {e}");
        }
    }

    if resume {
        // The suspect comes out of the roster as well as into the carried set: a
        // resume that ran it would submit a trainee the dead run may already
        // have submitted, which is the whole thing this module exists to stop.
        // Belt and braces since the promotion removed it from `pending`; the
        // filter is what keeps that true if the two ever drift apart.
        let suspect_id = suspect.as_ref().map(|record| record.trainee_id.as_str());

        return Ok(Start {
            plan: Plan::Resume {
                roster: previous
                    .never_attempted()
                    .iter()
                    .filter(|id| Some(id.as_str()) != suspect_id)
                    .map(|id| by_id(id))
                    .collect(),
                // Read off `previous` rather than off the `unreconciled` above,
                // so the count and the records it counts come from one snapshot.
                carried: carried(&previous),
                owed: unreconciled.indeterminate.len(),
            },
            previous: Some(previous),
            recovered,
        });
    }

    if blocked {
        return Err(refusal(
            store.path(),
            &previous,
            &unreconciled,
            suspect.as_ref(),
        ));
    }

    Ok(Start {
        plan: Plan::Fresh,
        previous: Some(previous),
        recovered,
    })
}

/// Whether the acknowledgement was asked for.
pub fn resume_requested() -> bool {
    std::env::var(RESUME_ENV).is_ok_and(|value| is_affirmative(&value))
}

/// Whether a value of [`RESUME_ENV`] is the operator saying yes.
///
/// Only an explicit affirmative counts. The gate exists to stop a run that could
/// resubmit a trainee, so an unrecognised value has to mean no: `DSSP_RESUME=off`
/// and a bare `DSSP_RESUME=` — which is what `DSSP_RESUME=$SOMETHING_UNSET`
/// produces — are each far more likely to be somebody declining, or a wrapper's
/// empty variable, than a considered yes. Reading either as consent is the one
/// mistake this module cannot afford, and the cost of the stricter reading is
/// only that a run is refused that would have been safe.
///
/// The asymmetry is the whole argument: a false yes can put a trainee on the
/// portal twice, a false no costs a re-run.
pub fn is_affirmative(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// A checkpoint holds ids, because that is what the engine queued. A resume
/// re-resolves them, so a trainee who has since left the portal stops the run
/// instead of being submitted against a dead id.
fn by_id(id: &str) -> TraineeRef {
    TraineeRef {
        id: Some(id.to_string()),
        name: None,
    }
}

/// Everything the predecessor settled or could not settle: the records a
/// resumed batch must carry into its own file rather than leave behind.
///
/// The engine rewrites the checkpoint in full, so anything left out of the first
/// write of a resumed batch is gone from the one artifact an operator has after
/// a crash. That includes the rows that *did* land: their absence would not
/// endanger a submission, but it would shrink `total` to the resumed subset and
/// turn the file from the account of a job into the account of a fragment of it
/// — and the report of the resumed run, which is built from the same rows, would
/// stop answering the question the operator actually has ("did all of them go
/// through?").
///
/// The never-attempted rows are deliberately absent: those are the roster, and a
/// trainee in both groups would be queued twice and reported twice.
fn carried(previous: &BatchCheckpoint) -> Vec<TrainingResult> {
    let mut records: Vec<TrainingResult> = previous
        .results
        .iter()
        .filter(|result| result.outcome != Outcome::Skipped)
        .cloned()
        .collect();

    // The suspect needs no separate argument: `guard` promotes it into
    // `in_flight` before calling, precisely so that there is one answer to "what
    // was left in the air?" rather than two that can disagree.
    records.extend(previous.in_flight_record());

    records
}

/// Records in `results` that were submitted and reached a conclusion.
///
/// Not "everything that is not unconfirmed". A `Skipped` row was drained after
/// an abort, so it was never sent — and [`BatchCheckpoint::never_attempted`]
/// already counts it as work still to do. Counting it here as well would put one
/// row in two groups: the refusal's breakdown would exceed `total`, and it would
/// tell an operator that more submissions reached the portal than were ever
/// attempted, in the single message they base a portal check on.
///
/// Also not `results.len() - unconfirmed.len()`: the in-flight trainee is
/// synthesized into the unconfirmed group without ever being in `results`, so
/// subtracting that count would under-report what settled — sometimes to zero,
/// for a batch whose only result landed.
fn settled(cp: &BatchCheckpoint) -> usize {
    cp.results
        .iter()
        .filter(|result| matches!(result.outcome, Outcome::Success | Outcome::Failed))
        .count()
}

/// The refusal an operator reads, naming the file, what is in it, why it cannot
/// be overwritten, and the ways forward.
fn refusal(
    path: &Path,
    previous: &BatchCheckpoint,
    unreconciled: &Unreconciled,
    suspect: Option<&TrainingResult>,
) -> String {
    // Not `never_attempted().len()` when there is a suspect: that trainee is
    // carried rather than continued, so counting it here would promise a resume
    // one more run than it will actually make.
    let suspect_id = suspect.map(|record| record.trainee_id.as_str());
    let never = previous
        .never_attempted()
        .iter()
        .filter(|id| Some(id.as_str()) != suspect_id)
        .count();
    let unconfirmed = unreconciled.indeterminate.len();

    let unaccounted = previous.unaccounted();

    let action = if unaccounted > 0 {
        // Not the resume advice: continuing would not tell the operator anything
        // about whoever is missing, and the message must not imply it would.
        format!(
            "the counts are short by {unaccounted} — check the portal before deciding anything; \
             a resume will not run them"
        )
    } else if never > 0 {
        format!(
            "re-run with {RESUME_ENV}=1 to continue the {never} never-attempted trainee(s); \
             nothing unconfirmed is replayed"
        )
    } else {
        format!(
            "re-run with {RESUME_ENV}=1 to carry the unconfirmed record(s) forward — \
             there is nothing left to attempt"
        )
    };

    format!(
        "dssp-bot: refusing to start — the checkpoint at {} is not reconciled\n\
         dssp-bot:   previous batch started {}: {} trainee(s), {} settled, \
         {unconfirmed} unconfirmed, {never} never attempted\n\
         dssp-bot:   {}\n\
         dssp-bot:   {action}\n\
         dssp-bot:   or move {} aside to start a fresh batch, once you have checked the portal",
        path.display(),
        previous.started_at,
        previous.total,
        settled(previous),
        reason(previous, unreconciled, suspect),
        path.display(),
    )
}

/// Why the file cannot simply be overwritten. Each case matters differently to
/// whoever has to look: a named trainee is a lookup, a count is a search, a file
/// too old to say who was in flight is a warning that its own queue is not the
/// evidence it looks like, and a file whose counts do not add up is a warning
/// that something is missing that the file cannot even name.
fn reason(
    previous: &BatchCheckpoint,
    unreconciled: &Unreconciled,
    suspect: Option<&TrainingResult>,
) -> String {
    // Ahead of the in-flight branch, because `guard` promotes the suspect into
    // `in_flight` before this is called: without the ordering it would be
    // described as an ordinary crash window, and the operator would lose the one
    // fact that changes how much the rest of the file can be trusted.
    if let Some(record) = suspect {
        return format!(
            "the file was written by a build that could not record a submission in flight, so \
             {} — next in its queue — may already be on the portal",
            record.trainee_id
        );
    }

    if let Some(id) = &previous.in_flight {
        return format!(
            "the process was killed while submitting {id} — that submission may have reached \
             the portal"
        );
    }

    if previous.unaccounted() > 0 {
        return format!(
            "it accounts for {} of its {} trainee(s), so {} cannot be placed at all — check the \
             portal for whoever is missing",
            previous.total - previous.unaccounted(),
            previous.total,
            previous.unaccounted(),
        );
    }

    // Reachable only with an unconfirmed record that is not an in-flight one:
    // `blocks_start` refuses over `has_unconfirmed` (an in-flight trainee, or an
    // `Indeterminate` row, both handled above) or over a count that does not add
    // up. Liveness alone no longer blocks, which is why the "the process was
    // killed, so a trainee may be missing" wording this branch replaced is gone.
    debug_assert!(
        !unreconciled.indeterminate.is_empty(),
        "a file with nothing unconfirmed and nothing unaccounted cannot be blocked"
    );

    format!(
        "{} submission(s) may already exist on the portal — check them there before going on",
        unreconciled.indeterminate.len()
    )
}

/// A predecessor was found and reconciled: its records are all still there.
pub fn recovered_line(path: &Path, previous: &BatchCheckpoint) -> String {
    let unconfirmed = previous.unreconciled().indeterminate.len();

    format!(
        "dssp-bot: recovered an interrupted batch from {}: {} settled, {unconfirmed} unconfirmed, \
         {} never attempted of {}",
        path.display(),
        settled(previous),
        previous.never_attempted().len(),
        previous.total,
    )
}

/// A file that exists but cannot be read is not a file that says nothing ran.
fn unreadable(path: &Path, error: &str) -> String {
    format!(
        "dssp-bot: refusing to start — the checkpoint at {} cannot be read: {error}\n\
         dssp-bot:   a checkpoint that cannot be read must not be treated as if no batch ran\n\
         dssp-bot:   if you are certain nothing was submitted, move {} aside and re-run",
        path.display(),
        path.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::CheckpointStatus;
    use crate::report::Outcome;
    use std::fs;
    use std::path::PathBuf;

    /// A unique directory under the system temp dir, so tests never collide.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dssp-bot-recovery-{}-{name}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&dir).expect("scratch dir");

            Self { dir }
        }

        fn store(&self) -> CheckpointStore {
            CheckpointStore::new(self.dir.join("dssp.checkpoint.json"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

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

    /// A batch that submitted two, could not confirm one, and left two queued.
    fn unfinished(status: CheckpointStatus) -> BatchCheckpoint {
        BatchCheckpoint {
            status,
            started_at: "1758285600000".to_string(),
            updated_at: "1758285900000".to_string(),
            total: 4,
            results: vec![
                result("1", Outcome::Success),
                result("2", Outcome::Indeterminate),
            ],
            pending: vec!["3".to_string(), "4".to_string()],
            in_flight: None,
        }
    }

    /// Killed between sending trainee 2 and hearing back: the window this stage
    /// exists to make visible.
    fn killed_mid_submission() -> BatchCheckpoint {
        BatchCheckpoint {
            status: CheckpointStatus::Running,
            started_at: "1758285600000".to_string(),
            updated_at: "1758285900000".to_string(),
            total: 4,
            results: vec![result("1", Outcome::Success)],
            pending: vec!["3".to_string(), "4".to_string()],
            in_flight: Some("2".to_string()),
        }
    }

    #[track_caller]
    fn assert_fresh(start: &Start) {
        assert!(matches!(start.plan, Plan::Fresh), "{:?}", start.plan);
    }

    // -- the gate -----------------------------------------------------------

    /// A first run is not a recovery.
    #[test]
    fn a_missing_checkpoint_starts_fresh() {
        let scratch = Scratch::new("missing");
        let start = guard(&scratch.store(), false).expect("nothing to refuse");

        assert_fresh(&start);
        assert!(start.previous.is_none());
        assert!(!start.recovered);
    }

    /// A batch whose records all landed owes nothing, so its slot may be reused
    /// without the operator having to move a file.
    #[test]
    fn a_settled_batch_does_not_refuse_the_next_one() {
        let scratch = Scratch::new("settled");
        let store = scratch.store();
        let mut done = unfinished(CheckpointStatus::Finished);
        done.total = 2;
        done.results = vec![result("1", Outcome::Success), result("2", Outcome::Failed)];
        done.pending = Vec::new();
        store.save(&done).expect("saves");

        let start = guard(&store, false).expect("nothing to refuse");

        assert_fresh(&start);
        assert!(!start.recovered, "it was already terminal");
        assert!(start.previous.unwrap().never_attempted().is_empty());
    }

    /// The gap Stage 27 filed: the next batch's first write would have replaced
    /// this file, and nothing would have said so.
    #[test]
    fn a_live_checkpoint_refuses_a_start() {
        let scratch = Scratch::new("live");
        let store = scratch.store();
        store.save(&unfinished(CheckpointStatus::Running)).expect("saves");

        let refusal = guard(&store, false).expect_err("a killed run must not be overwritten");

        assert!(refusal.contains("refusing to start"), "{refusal}");
        assert!(refusal.contains("dssp.checkpoint.json"), "{refusal}");
    }

    /// Refusing is not enough on its own: the file has to record the conclusion,
    /// or the next start refuses for the same reason with no more information.
    #[test]
    fn a_refused_start_marks_the_live_checkpoint_interrupted() {
        let scratch = Scratch::new("mark");
        let store = scratch.store();
        store.save(&unfinished(CheckpointStatus::Running)).expect("saves");

        let _ = guard(&store, false).expect_err("refused");

        let after = store.load().expect("reads").expect("still there");

        assert_eq!(after.status, CheckpointStatus::Interrupted);
        assert!(!after.is_live());
    }

    /// A file left live but owing nothing — the process died between its last
    /// settle and the terminal write. Nothing in it is in doubt: `in_flight` is
    /// empty, so no submission was outstanding when it died. It therefore starts
    /// clean, and says what it recovered rather than making the operator
    /// acknowledge a file that has nothing to acknowledge.
    ///
    /// Liveness alone used to block this, which made the gate cry wolf on every
    /// ordinary kill-and-retry — and a gate an operator learns to answer with
    /// `DSSP_RESUME=1` is one that stops being read.
    #[test]
    fn a_live_file_that_owes_nothing_starts_clean_and_says_what_it_recovered() {
        let scratch = Scratch::new("live-nothing-owed");
        let store = scratch.store();
        let mut cp = unfinished(CheckpointStatus::Running);
        cp.total = 2;
        cp.results = vec![result("1", Outcome::Success), result("2", Outcome::Success)];
        cp.pending = Vec::new();
        store.save(&cp).expect("saves");

        let start = guard(&store, false).expect("nothing is in doubt");

        assert_fresh(&start);
        assert!(start.recovered, "but the kill is still recorded and reported");

        // And the mark is durable, so the next start is an ordinary one.
        let after = CheckpointStore::new(store.path()).load().expect("reads").expect("there");
        assert_eq!(after.status, CheckpointStatus::Interrupted);
    }

    /// The regression for the hole this gate had: the refusal's own write used
    /// to erase the evidence it had just refused over.
    ///
    /// A file from a build without in-flight tracking says what it knows by
    /// *omitting* the key, and every write from this build adds it. So the
    /// refusal — which rewrites the file to record the interruption — turned a
    /// legacy file into one that looked current, and the follow-up run the
    /// refusal itself recommends then found no suspect and put the trainee the
    /// dead build may already have sent back into the roster.
    #[test]
    fn refusing_a_legacy_file_does_not_erase_the_suspicion_it_refused_over() {
        let scratch = Scratch::new("legacy-survives");
        let store = scratch.store();
        let mut cp = unfinished(CheckpointStatus::Running);
        cp.total = 3;
        cp.results = vec![result("1", Outcome::Success)];
        cp.pending = vec!["2".to_string(), "3".to_string()];
        cp.in_flight = None;
        store.save(&cp).expect("saves");

        // Written by this build, the file carries the key; strip it so the file
        // is what a pre-Stage-28 build would have left.
        let text = std::fs::read_to_string(store.path()).expect("reads");
        let raw: serde_json::Value = serde_json::from_str(&text).expect("parses");
        let mut object = raw.as_object().expect("an object").clone();
        object.remove("in_flight");
        std::fs::write(
            store.path(),
            serde_json::to_string(&serde_json::Value::Object(object)).expect("serializes"),
        )
        .expect("writes");

        // Run 1: refused, and the refusal records the interruption.
        let refusal = guard(&store, false).expect_err("a legacy file with a queue is refused");
        assert!(refusal.contains("could not record a submission in flight"), "{refusal}");
        assert!(refusal.contains('2'), "and it names the trainee: {refusal}");

        // Run 2, exactly as the refusal advises: trainee 2 must not be run.
        let start = guard(&store, true).expect("the override");
        let Plan::Resume { roster, carried, owed } = start.plan else {
            panic!("expected a resume");
        };

        let ids: Vec<&str> = roster.iter().filter_map(|r| r.id.as_deref()).collect();
        assert_eq!(ids, vec!["3"], "only the trainee the dead build never reached");
        assert!(carried.iter().any(|r| r.trainee_id == "2"), "2 is carried, not run");
        assert_eq!(owed, 1, "and it is still owed an answer");
    }

    /// A file that lost a trainee outright — its counts do not add up, and
    /// nothing in it says who is missing. That is reason enough to refuse on its
    /// own, because the file cannot testify to its own completeness.
    #[test]
    fn a_file_that_lost_a_trainee_refuses_a_start() {
        let scratch = Scratch::new("unaccounted");
        let store = scratch.store();
        let mut cp = unfinished(CheckpointStatus::Interrupted);
        cp.total = 4;
        cp.results = vec![result("1", Outcome::Success)];
        cp.pending = Vec::new();
        cp.in_flight = None;
        store.save(&cp).expect("saves");

        let refusal = guard(&store, false).expect_err("it accounts for 1 of 4");
        assert!(refusal.contains("accounts for 1 of its 4"), "{refusal}");
        assert!(refusal.contains("check the portal"), "{refusal}");

        // A resume is still allowed — it runs nothing outside `never_attempted`,
        // so it cannot resubmit whoever is missing — but it must not pretend the
        // file is whole.
        let start = guard(&store, true).expect("safe to continue what it does know");
        let Plan::Resume { roster, .. } = start.plan else {
            panic!("expected a resume");
        };

        assert!(roster.is_empty(), "there was nothing left to attempt");
    }

    /// `DSSP_RESUME` is set by hand, often from a shell history entry — and
    /// often from a variable that turns out to be empty. Only an explicit yes
    /// resumes, because a false yes can submit a trainee twice and a false no
    /// only costs a re-run.
    #[test]
    fn only_an_explicit_yes_counts_as_yes() {
        for on in ["1", "true", "yes", "on", "TRUE", " yes ", "On"] {
            assert!(is_affirmative(on), "{on:?} should be on");
        }

        for off in [
            "0", "false", "no", "FALSE", " 0 ", "No", // the documented negatives
            "", "  ", "off", "disabled", "none", "n", "y", "2", "maybe",
        ] {
            assert!(!is_affirmative(off), "{off:?} should be off");
        }
    }

    /// Lossless: the mark rewrites the status, never the record. Anything else
    /// would destroy the very account the refusal is telling the operator to
    /// read.
    #[test]
    fn a_refusal_leaves_every_record_intact() {
        let scratch = Scratch::new("lossless");
        let store = scratch.store();
        let before = unfinished(CheckpointStatus::Running);
        store.save(&before).expect("saves");

        let _ = guard(&store, false).expect_err("refused");

        let after = store.load().expect("reads").expect("still there");

        assert_eq!(after.results.len(), before.results.len());
        assert_eq!(after.results[1].outcome, Outcome::Indeterminate);
        assert_eq!(after.pending, before.pending);
        assert_eq!(after.total, before.total);
        assert_eq!(after.started_at, before.started_at);
        assert_eq!(after.in_flight, before.in_flight);
    }

    /// A terminal status is not enough: this record may already exist on the
    /// portal, and the new batch's first write would drop it.
    #[test]
    fn an_unconfirmed_record_refuses_a_start_from_a_finished_batch() {        let scratch = Scratch::new("unconfirmed");
        let store = scratch.store();
        store.save(&unfinished(CheckpointStatus::Finished)).expect("saves");

        let refusal = guard(&store, false).expect_err("must not be overwritten");

        assert!(refusal.contains("may already exist on the portal"), "{refusal}");
        assert!(refusal.contains("DSSP_RESUME=1"), "{refusal}");
    }

    /// The crash window, end to end: a run killed between sending and hearing
    /// back must not be able to have that trainee submitted again.
    #[test]
    fn the_crash_window_trainee_refuses_a_start_and_is_named() {
        let scratch = Scratch::new("in-flight");
        let store = scratch.store();
        store.save(&killed_mid_submission()).expect("saves");

        let refusal = guard(&store, false).expect_err("must not be overwritten");

        assert!(refusal.contains("killed while submitting 2"), "{refusal}");
        assert!(refusal.contains("may have reached the portal"), "{refusal}");
    }

    /// A corrupt file cannot be allowed to read as "no batch ran" — the one
    /// conclusion that invites a second submission.
    #[test]
    fn an_unreadable_checkpoint_refuses_a_start() {        let scratch = Scratch::new("corrupt");
        let store = scratch.store();
        fs::write(store.path(), r#"{"status":"running","resul"#).expect("write partial json");

        let refusal = guard(&store, false).expect_err("must not be read as nothing");

        assert!(refusal.contains("cannot be read"), "{refusal}");
        assert!(!refusal.contains("DSSP_RESUME"), "resuming cannot fix it: {refusal}");
    }

    /// The acknowledgement must not be a way to overwrite an unreadable file
    /// either: there is nothing to resume from.
    #[test]
    fn an_unreadable_checkpoint_refuses_even_a_resume() {
        let scratch = Scratch::new("corrupt-resume");
        let store = scratch.store();
        fs::write(store.path(), "not json at all").expect("write");

        assert!(guard(&store, true).is_err());
    }

    // -- resume -------------------------------------------------------------

    /// The acknowledgement buys the never-attempted trainees and nothing else.
    #[test]
    fn a_resume_continues_the_never_attempted_trainees() {
        let scratch = Scratch::new("resume-roster");
        let store = scratch.store();
        let mut previous = unfinished(CheckpointStatus::Finished);
        previous.results.push(result("5", Outcome::Skipped));
        store.save(&previous).expect("saves");

        let start = guard(&store, true).expect("the override");

        match start.plan {
            Plan::Resume { roster, carried, owed } => {
                let ids: Vec<String> = roster.iter().filter_map(|r| r.id.clone()).collect();

                assert_eq!(ids, vec!["5", "3", "4"], "drained skips, then the queue");
                assert!(roster.iter().all(|r| r.name.is_none()), "resolved by id");

                // History, not work: the two that settled and the one that could
                // not are carried, the skipped one is not (it is in the roster).
                let carried_ids: Vec<String> =
                    carried.iter().map(|r| r.trainee_id.clone()).collect();
                assert_eq!(carried_ids, vec!["1", "2"]);
                assert_eq!(owed, 1, "only the unconfirmed one is still owed");
            }
            other => panic!("expected a resume, got {other:?}"),
        }
    }

    /// The resumed run's own accounting has to add up to the batch it continues.
    /// If it does not, `total` — the one number nothing recomputes — starts
    /// reporting the fragment instead of the job.
    #[test]
    fn a_resume_accounts_for_exactly_the_batch_it_continues() {
        let scratch = Scratch::new("accounting");
        let store = scratch.store();
        let previous = killed_mid_submission();
        let total = previous.total;
        store.save(&previous).expect("saves");

        let start = guard(&store, true).expect("the override");

        match start.plan {
            Plan::Resume { roster, carried, .. } => {
                assert_eq!(roster.len() + carried.len(), total);
            }
            other => panic!("expected a resume, got {other:?}"),
        }
    }

    /// "Indeterminate jobs aren't replayed" — the rule the whole design turns
    /// on, asserted against the roster the engine will actually be handed.
    #[test]
    fn a_resume_never_offers_an_unconfirmed_trainee() {
        let scratch = Scratch::new("no-replay");
        let store = scratch.store();
        store.save(&unfinished(CheckpointStatus::Finished)).expect("saves");

        let start = guard(&store, true).expect("the override");
        let (roster, carried, _) = match start.plan {
            Plan::Resume { roster, carried, owed } => (roster, carried, owed),
            other => panic!("expected a resume, got {other:?}"),
        };
        let ids: Vec<String> = roster.iter().filter_map(|r| r.id.clone()).collect();

        assert!(!ids.contains(&"2".to_string()), "{ids:?}");
        assert!(carried.iter().any(|r| r.trainee_id == "2"));
    }

    /// And neither is the crashed one: it is carried, never queued.
    #[test]
    fn a_resume_never_offers_the_crash_window_trainee() {
        let scratch = Scratch::new("no-replay-in-flight");
        let store = scratch.store();
        store.save(&killed_mid_submission()).expect("saves");

        let start = guard(&store, true).expect("the override");
        let (roster, carried, _) = match start.plan {
            Plan::Resume { roster, carried, owed } => (roster, carried, owed),
            other => panic!("expected a resume, got {other:?}"),
        };
        let ids: Vec<String> = roster.iter().filter_map(|r| r.id.clone()).collect();

        assert_eq!(ids, vec!["3", "4"], "the never-attempted, and only those");
        assert_eq!(carried.len(), 2, "the one that settled, and the one in flight");

        let in_flight = carried
            .iter()
            .find(|r| r.trainee_id == "2")
            .expect("the crash-window trainee is carried");
        assert_eq!(
            in_flight.error_code.as_deref(),
            Some(crate::checkpoint::IN_FLIGHT_CODE)
        );
    }

    /// Already-recorded trainees are neither carried nor re-run.
    #[test]
    fn a_resume_skips_what_already_settled() {
        let scratch = Scratch::new("settled-skip");
        let store = scratch.store();
        store.save(&unfinished(CheckpointStatus::Finished)).expect("saves");

        let start = guard(&store, true).expect("the override");
        let roster = match start.plan {
            Plan::Resume { roster, .. } => roster,
            other => panic!("expected a resume, got {other:?}"),
        };
        let ids: Vec<String> = roster.iter().filter_map(|r| r.id.clone()).collect();

        assert!(!ids.contains(&"1".to_string()), "settled: {ids:?}");
    }

    /// A reconciled checkpoint with nothing unconfirmed and nothing un-attempted
    /// resumes to an empty roster: the caller reports that rather than running an
    /// empty batch. The settled row still rides along, because the file it is
    /// resumed into is rewritten in full and this run should not be the reason
    /// that record disappears.
    #[test]
    fn a_resume_of_a_settled_batch_has_nothing_to_run() {
        let scratch = Scratch::new("clean-resume");
        let store = scratch.store();
        let mut done = unfinished(CheckpointStatus::Interrupted);
        done.results = vec![result("1", Outcome::Success)];
        done.pending = Vec::new();
        store.save(&done).expect("saves");

        let start = guard(&store, true).expect("the override");

        match start.plan {
            Plan::Resume { roster, carried, owed } => {
                assert!(roster.is_empty(), "nothing was left unsent");
                assert_eq!(carried.len(), 1, "the record that did land");
                assert_eq!(owed, 0, "and nothing is owed: exit 0, not 3");
            }
            other => panic!("expected a resume, got {other:?}"),
        }
    }

    // -- files from a build without in-flight tracking ----------------------

    /// A file old enough to predate the marker. Its head-of-queue is the trainee
    /// the dead build may have been submitting, and the typed parse cannot tell
    /// this file from a current one that happens to be idle.
    fn untracked(scratch: &Scratch) -> CheckpointStore {
        let store = scratch.store();
        fs::write(
            store.path(),
            r#"{
  "status": "running",
  "started_at": "1758285600000",
  "updated_at": "1758285900000",
  "total": 4,
  "results": [
    {
      "trainee_id": "1",
      "trainee_name": "Trainee 1",
      "outcome": "success",
      "attempts": 1
    }
  ],
  "pending": ["2", "3", "4"]
}"#,
        )
        .expect("write an older checkpoint");

        store
    }

    /// The hole this closes: on such a file the head of the queue may already be
    /// on the portal, and a roster built from `pending` alone would submit it
    /// again — the duplicate Gate 3 forbids, caused by a file, not a crash.
    #[test]
    fn an_untracked_file_refuses_and_names_the_trainee_it_cannot_account_for() {
        let scratch = Scratch::new("legacy-refuse");
        let store = untracked(&scratch);

        let refusal = guard(&store, false).expect_err("must not be overwritten");

        assert!(refusal.contains("could not record a submission in flight"), "{refusal}");
        assert!(refusal.contains("may already be on the portal"), "{refusal}");
        assert!(refusal.contains('2'), "the trainee in question: {refusal}");
        assert!(
            refusal.contains("2 never attempted"),
            "the suspect is not one a resume promises to run: {refusal}"
        );
    }

    /// And a resume keeps it out of the queue, while still continuing the two
    /// behind it — the file is written off, not the batch.
    #[test]
    fn a_resume_of_an_untracked_file_runs_the_queue_behind_the_suspect() {
        let scratch = Scratch::new("legacy-resume");
        let store = untracked(&scratch);

        let start = guard(&store, true).expect("the override");

        match start.plan {
            Plan::Resume { roster, carried, owed } => {
                let ids: Vec<String> = roster.iter().filter_map(|r| r.id.clone()).collect();

                assert_eq!(ids, vec!["3", "4"], "not the one that may have been sent");
                assert!(carried.iter().any(|r| r.trainee_id == "2"));
                assert!(
                    carried.iter().any(|r| r.trainee_id == "1"),
                    "the row that landed rides along too"
                );
                assert_eq!(owed, 1, "the suspect is what a human is owed");
            }
            other => panic!("expected a resume, got {other:?}"),
        }
    }

    /// A current file with the same queue is not suspected of anything: its
    /// `in_flight` is null precisely because nothing was handed to the worker.
    #[test]
    fn a_tracked_file_with_a_queue_is_not_suspected() {
        let scratch = Scratch::new("tracked-queue");
        let store = scratch.store();
        let mut cp = unfinished(CheckpointStatus::Finished);
        cp.results = vec![result("1", Outcome::Success)];
        cp.pending = vec!["2".to_string()];
        store.save(&cp).expect("saves");

        let start = guard(&store, true).expect("the override");

        match start.plan {
            Plan::Resume { roster, .. } => {
                let ids: Vec<String> = roster.iter().filter_map(|r| r.id.clone()).collect();

                assert_eq!(ids, vec!["2"], "its whole queue is continueable");
            }
            other => panic!("expected a resume, got {other:?}"),
        }
    }

    // -- messages -----------------------------------------------------------

    /// The refusal is the interface: it has to name the file, say what is in it,
    /// and offer both ways forward.
    #[test]
    fn the_refusal_names_the_file_the_counts_and_the_ways_forward() {
        let scratch = Scratch::new("refusal-text");
        let store = scratch.store();
        store.save(&unfinished(CheckpointStatus::Finished)).expect("saves");

        let refusal = guard(&store, false).expect_err("refused");

        assert!(refusal.contains("dssp.checkpoint.json"), "{refusal}");
        assert!(refusal.contains("1758285600000"), "the batch's clock: {refusal}");
        assert!(refusal.contains("4 trainee(s)"), "{refusal}");
        assert!(refusal.contains("1 settled"), "not the in-flight count: {refusal}");
        assert!(refusal.contains("1 unconfirmed"), "{refusal}");
        assert!(refusal.contains("2 never attempted"), "{refusal}");
        assert!(refusal.contains("DSSP_RESUME=1"), "{refusal}");
        assert!(refusal.contains("move"), "the way out: {refusal}");
    }

    /// The counts have to be right when the unconfirmed record is the
    /// synthesized one, which is in no `results` entry to be subtracted.
    #[test]
    fn the_refusal_counts_a_crash_window_trainee_separately_from_the_settled_ones() {
        let scratch = Scratch::new("refusal-in-flight");
        let store = scratch.store();
        store.save(&killed_mid_submission()).expect("saves");

        let refusal = guard(&store, false).expect_err("refused");

        assert!(refusal.contains("1 settled"), "{refusal}");
        assert!(refusal.contains("1 unconfirmed"), "{refusal}");
    }

    #[test]
    fn the_recovered_line_reports_what_the_dead_batch_left() {
        let scratch = Scratch::new("recovered-text");
        let store = scratch.store();
        store.save(&killed_mid_submission()).expect("saves");

        let start = guard(&store, true).expect("the override");
        let line = recovered_line(store.path(), start.previous.as_ref().unwrap());

        assert!(line.contains("dssp.checkpoint.json"), "{line}");
        assert!(line.contains("1 settled"), "{line}");
        assert!(line.contains("1 unconfirmed"), "{line}");
        assert!(line.contains("2 never attempted"), "{line}");
        assert!(line.contains("of 4"), "{line}");
    }
}