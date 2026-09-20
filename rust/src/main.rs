//! DSSP-Bot coordinator — one worker, one protocol, two entry points.
//!
//! * **single** (`"trainee"` in the job file) — the runbook's live gates. One
//!   submission, one classified result line, driven by [`decide_submit`].
//! * **batch** (`"trainees"`) — the [`BatchEngine`] establishes the session
//!   once, runs every trainee through the retry policy, and prints a
//!   [`BatchReport`]. Driven by `settle_submit`.
//!
//! Both decision paths live in `decision.rs`; Rust owns every retry/stop
//! decision either way, and the worker only reports.
//!
//! The protocol layer is intentionally complete ahead of full use; unused
//! members are expected while later stages land.
#![allow(dead_code)]

mod checkpoint;
mod decision;
mod engine;
mod protocol;
mod queue;
mod report;
mod state;
mod store;
mod worker;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::exit;

use decision::{decide_submit, Decision, RetryPolicy};
use engine::BatchEngine;
use protocol::{Request, SessionInput, Status, TraineeInfo, TraineeRef};
use report::BatchReport;
use serde::Deserialize;
use store::CheckpointStore;
use worker::WorkerClient;

#[derive(Deserialize)]
struct Job {
    /// Single-trainee form, used by the runbook's live gates (Stages 15, 24).
    #[serde(default)]
    trainee: Option<TraineeRef>,
    /// Batch form; mutually exclusive with `trainee`.
    #[serde(default)]
    trainees: Option<Vec<TraineeRef>>,
    session: SessionInput,
}

/// What the job file asked for, after validation.
enum Plan {
    Single(TraineeRef),
    Batch(Vec<TraineeRef>),
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Spawn the worker and block until it reports ready.
fn connect() -> Result<WorkerClient, String> {
    let mut client = WorkerClient::spawn()?;
    client.wait_ready()?;
    Ok(client)
}

/// Stable identity for a trainee reference — the id wins, else the raw name.
fn key_of(trainee: &TraineeRef) -> Option<String> {
    trainee.id.clone().or_else(|| trainee.name.clone())
}

/// Collapse whitespace and upper-case, so "John  Doe" == "JOHN DOE".
/// Mirrors `normalize_name` in python/app/portal/parsing.py.
fn normalize_name(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

/// Decide single vs batch from the job file.
///
/// Everything that can be checked without the portal is checked here, before a
/// browser opens: a malformed batch must fail closed rather than submit a
/// prefix of its trainees and then discover the problem. Names are resolved —
/// and duplicates dropped — later, in [`resolve_trainees`], once the worker can
/// be asked what the portal actually holds.
fn plan(job: &Job) -> Result<Plan, String> {
    match (&job.trainee, &job.trainees) {
        (Some(_), Some(_)) => {
            Err("job has both 'trainee' and 'trainees' — use exactly one".to_string())
        }
        (Some(trainee), None) => {
            key_of(trainee).ok_or_else(|| "'trainee' needs an 'id' or a 'name'".to_string())?;
            Ok(Plan::Single(trainee.clone()))
        }
        (None, Some(trainees)) => {
            if trainees.is_empty() {
                return Err("'trainees' is empty — nothing to submit".to_string());
            }
            for (index, trainee) in trainees.iter().enumerate() {
                if key_of(trainee).is_none() {
                    return Err(format!("trainees[{index}] needs an 'id' or a 'name'"));
                }
            }
            Ok(Plan::Batch(trainees.clone()))
        }
        (None, None) => Err("job needs 'trainee' or 'trainees'".to_string()),
    }
}

/// Why a batch could not be canonicalized.
enum ResolveFailure {
    /// The portal could not be read — an operator has to look, as with any HALT.
    Halt(String),
    /// The batch itself is unusable. Nothing was submitted.
    BadBatch(String),
}

impl ResolveFailure {
    fn exit_code(&self) -> i32 {
        match self {
            ResolveFailure::Halt(_) => 3,
            ResolveFailure::BadBatch(_) => 2,
        }
    }

    fn message(&self) -> &str {
        match self {
            ResolveFailure::Halt(message) | ResolveFailure::BadBatch(message) => message,
        }
    }
}

/// Resolve a batch against an already-fetched trainee list, then dedupe.
///
/// Mirrors `PortalClient._resolve_trainee` so a reference resolves the same way
/// whether the CLI or the worker does it: an explicit id must exist, a name
/// must match exactly one trainee, and anything else stops the batch while the
/// report is still empty. Deduping *after* resolution is the point — two
/// entries naming one person, one by id and one by name, collapse into a single
/// submission instead of the second coming back as a duplicate.
///
/// Every unresolvable entry is collected rather than stopping at the first, so
/// one run tells the operator about all of them. Split out from
/// [`resolve_trainees`] so the matching rules are testable without a worker.
fn resolve_against(
    available: &[TraineeInfo],
    trainees: &[TraineeRef],
) -> Result<Vec<TraineeRef>, Vec<String>> {
    let mut by_id: HashMap<&str, &TraineeInfo> = HashMap::new();
    let mut by_name: HashMap<String, Vec<&TraineeInfo>> = HashMap::new();
    for trainee in available {
        by_id.insert(trainee.id.as_str(), trainee);
        by_name
            .entry(normalize_name(&trainee.name))
            .or_default()
            .push(trainee);
    }

    let mut resolved: Vec<TraineeRef> = Vec::with_capacity(trainees.len());
    let mut seen: HashSet<String> = HashSet::new();
    let mut problems: Vec<String> = Vec::new();

    for (index, entry) in trainees.iter().enumerate() {
        let label = key_of(entry).unwrap_or_else(|| format!("#{index}"));

        // An explicit id wins over a name, exactly as the worker's resolver does.
        let found = match entry.id.as_deref() {
            Some(id) => match by_id.get(id).copied() {
                Some(found) => Ok(found),
                None => Err(format!("{label}: no trainee with that id on the portal")),
            },
            None => {
                let name = entry.name.clone().unwrap_or_default();
                let matches = by_name
                    .get(&normalize_name(&name))
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                match matches.len() {
                    1 => Ok(matches[0]),
                    0 => Err(format!("{label}: no trainee with that name on the portal")),
                    count => Err(format!(
                        "{label}: {count} trainees share that name — give the trainee id"
                    )),
                }
            }
        };

        match found {
            Ok(found) => {
                // Dedupe on the canonical id, so an id and a name for the same
                // person cannot both be submitted.
                if seen.insert(found.id.clone()) {
                    resolved.push(TraineeRef {
                        id: Some(found.id.clone()),
                        name: None,
                    });
                } else {
                    eprintln!(
                        "dssp-bot: {label}: already queued as id {} — skipping",
                        found.id
                    );
                }
            }
            Err(problem) => problems.push(problem),
        }
    }

    if problems.is_empty() {
        Ok(resolved)
    } else {
        Err(problems)
    }
}

/// Fetch the portal's trainee list and canonicalize the batch against it.
///
/// Resolution needs an authenticated session, so the caller establishes one
/// before calling this.
fn resolve_trainees(
    client: &mut WorkerClient,
    trainees: &[TraineeRef],
) -> Result<Vec<TraineeRef>, ResolveFailure> {
    let resp = client
        .send(&Request::list_trainees(new_id()))
        .map_err(|e| ResolveFailure::Halt(format!("could not list trainees: {e}")))?;

    if resp.status != Status::Ok {
        // A lapsed session or a changed portal needs a human, not a retry.
        return Err(ResolveFailure::Halt(format!(
            "could not list trainees: {}",
            resp.summary()
        )));
    }

    resolve_against(&resp.trainees.unwrap_or_default(), trainees).map_err(|problems| {
        ResolveFailure::BadBatch(format!(
            "cannot resolve {} of {} trainee(s):\n  - {}",
            problems.len(),
            trainees.len(),
            problems.join("\n  - ")
        ))
    })
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "job.json".to_string());
    let job: Job = match fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(job) => job,
        Err(e) => {
            eprintln!("dssp-bot: cannot read job {path}: {e}");
            exit(2);
        }
    };

    let code = match plan(&job) {
        Ok(Plan::Single(trainee)) => run_single(trainee, &job.session),
        Ok(Plan::Batch(trainees)) => run_batch(&trainees, &job.session, &path),
        Err(e) => {
            eprintln!("dssp-bot: {e}");
            exit(2);
        }
    };

    exit(code);
}

/// One trainee end to end: session gate, informational form options, then a
/// single submission with Rust-owned retry. Prints one classified line.
fn run_single(trainee: TraineeRef, session: &SessionInput) -> i32 {
    let mut client = match connect() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("dssp-bot: {e}");
            return 1;
        }
    };

    // 1. Session gate. On the first run this blocks while you log in manually
    //    in the opened browser window.
    eprintln!("dssp-bot: ensuring portal session…");
    match client.send(&Request::ensure_session(new_id())) {
        Ok(r) if r.status == Status::Ok => eprintln!("dssp-bot: session ok"),
        Ok(r) => {
            eprintln!("dssp-bot: session failed: {}", r.summary());
            return 1;
        }
        Err(e) => {
            eprintln!("dssp-bot: session error: {e}");
            return 1;
        }
    }

    // 2. Informational: show the options the portal actually offers.
    match client.send(&Request::get_form_options(new_id(), None)) {
        Ok(r) if r.status == Status::Ok => eprintln!(
            "dssp-bot: form options — {} instructors, {} training types",
            r.instructors.as_ref().map_or(0, Vec::len),
            r.training_types.as_ref().map_or(0, Vec::len),
        ),
        Ok(r) => eprintln!("dssp-bot: form options unavailable: {}", r.summary()),
        Err(e) => eprintln!("dssp-bot: form options error: {e}"),
    }

    // 3. Submit one trainee. Rust owns retry; the same job_id spans attempts.
    let policy = RetryPolicy::default();
    let job_id = new_id();
    let mut attempt = 0u32;

    let (line, code) = loop {
        attempt += 1;
        let req =
            Request::submit_training(job_id.clone(), trainee.clone(), session.clone());

        let resp = match client.send(&req) {
            Ok(resp) => resp,
            Err(e) => break (format!("HALT: transport error, submission state unknown: {e}"), 3),
        };

        match decide_submit(&resp) {
            Decision::Accept(s) => break (format!("RESULT: {s}"), 0),
            Decision::Reject(s) => break (format!("FAILED: {s}"), 4),
            Decision::Halt(s) => break (format!("HALT: {s}"), 3),
            Decision::Retry(s) => {
                if attempt >= policy.max_attempts {
                    break (format!("FAILED: gave up after {attempt} attempts — {s}"), 4);
                }
                let delay = policy.delay_for_attempt(attempt);
                eprintln!(
                    "dssp-bot: attempt {attempt} retryable ({s}); retrying in {} ms",
                    delay.as_millis()
                );
                std::thread::sleep(delay);
            }
        }
    };

    client.shutdown();
    println!("{line}");
    code
}

/// Many trainees, one session. The engine aborts the whole batch — draining
/// whatever is still queued as skipped — the moment a result cannot be
/// accounted for.
///
/// Unlike the single-trainee path, a batch always checkpoints: it is the run
/// most likely to be interrupted, and the one where an interrupted run has real
/// records on the portal to account for. A single submission that dies leaves
/// one line of terminal output and nothing ambiguous behind it.
fn run_batch(trainees: &[TraineeRef], session: &SessionInput, job_path: &str) -> i32 {
    let mut client = match connect() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("dssp-bot: {e}");
            return 1;
        }
    };

    // The trainee list has to be readable before the batch can be canonicalized,
    // so the session gate comes first; the engine re-checks it for the run.
    eprintln!("dssp-bot: ensuring portal session…");
    match client.send(&Request::ensure_session(new_id())) {
        Ok(r) if r.status == Status::Ok => eprintln!("dssp-bot: session ok"),
        Ok(r) => {
            eprintln!("dssp-bot: session failed: {}", r.summary());
            client.shutdown();
            return 3;
        }
        Err(e) => {
            eprintln!("dssp-bot: session error: {e}");
            client.shutdown();
            return 3;
        }
    }

    let resolved = match resolve_trainees(&mut client, trainees) {
        Ok(resolved) => resolved,
        Err(failure) => {
            eprintln!("dssp-bot: {}", failure.message());
            client.shutdown();
            return failure.exit_code();
        }
    };

    eprintln!("dssp-bot: batch of {} trainee(s)", resolved.len());

    // Where a killed batch leaves its record. Named on stderr because the file
    // is the operator's only account of a run that never printed a report.
    let checkpoint = CheckpointStore::for_job(Path::new(job_path));
    eprintln!("dssp-bot: checkpoint → {}", checkpoint.path().display());

    let mut engine = BatchEngine::default().with_checkpoint(Box::new(checkpoint));
    let report = match engine.run(&mut client, session, resolved) {
        Ok(report) => report,
        Err(e) => {
            // The session could not be established, so nothing was written.
            eprintln!("dssp-bot: batch aborted before any submission: {e}");
            client.shutdown();
            return 3;
        }
    };

    client.shutdown();
    print_report(&report);
    batch_exit_code(&report)
}

/// Progress on stderr; the machine-readable report alone on stdout.
fn print_report(report: &BatchReport) {
    for result in &report.results {
        let who = if result.trainee_name.is_empty() {
            &result.trainee_id
        } else {
            &result.trainee_name
        };
        eprintln!(
            "dssp-bot: {:?} {who} (attempts={}) {}",
            result.outcome,
            result.attempts,
            result.message.as_deref().unwrap_or("")
        );
    }
    eprintln!("dssp-bot: {}", report.summary_line());

    match serde_json::to_string_pretty(report) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("dssp-bot: could not serialize report: {e}"),
    }
}

/// An indeterminate record may have been written without a readable
/// confirmation, and a skipped one sits behind an aborted batch — both need a
/// human. A definitive failure is terminal but safe to re-run.
fn batch_exit_code(report: &BatchReport) -> i32 {
    if report.indeterminate > 0 || report.skipped > 0 {
        3
    } else if report.failed > 0 {
        4
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portal(rows: &[(&str, &str)]) -> Vec<TraineeInfo> {
        rows.iter()
            .map(|(id, name)| TraineeInfo {
                id: (*id).to_string(),
                name: (*name).to_string(),
                sn: String::new(),
                course: String::new(),
                training_sessions: String::new(),
            })
            .collect()
    }

    fn want_id(id: &str) -> TraineeRef {
        TraineeRef {
            id: Some(id.to_string()),
            name: None,
        }
    }

    fn want_name(name: &str) -> TraineeRef {
        TraineeRef {
            id: None,
            name: Some(name.to_string()),
        }
    }

    /// Resolved ids, in order — resolved entries always carry an id and no name.
    fn ids(refs: &[TraineeRef]) -> Vec<String> {
        refs.iter()
            .map(|r| {
                assert!(r.name.is_none(), "resolved entries submit by id only");
                r.id.clone().unwrap_or_default()
            })
            .collect()
    }

    fn problems(result: Result<Vec<TraineeRef>, Vec<String>>) -> String {
        result.expect_err("expected a resolution failure").join("; ")
    }

    #[test]
    fn normalize_name_collapses_whitespace_and_case() {
        assert_eq!(normalize_name("  John   Doe "), "JOHN DOE");
        assert_eq!(normalize_name("jane roe"), "JANE ROE");
    }

    #[test]
    fn a_name_resolves_to_its_id() {
        let available = portal(&[("123", "John Doe"), ("456", "Jane Roe")]);
        let resolved = resolve_against(&available, &[want_name("john doe")]).unwrap();
        assert_eq!(ids(&resolved), vec!["123"]);
    }

    #[test]
    fn an_unknown_name_is_reported() {
        let available = portal(&[("123", "John Doe")]);
        let message = problems(resolve_against(&available, &[want_name("Nobody Here")]));
        assert!(message.contains("Nobody Here"), "{message}");
        assert!(message.contains("no trainee with that name"), "{message}");
    }

    /// The never-guess rule: two trainees sharing a name must not pick one.
    #[test]
    fn an_ambiguous_name_is_reported_rather_than_guessed() {
        let available = portal(&[("123", "John Doe"), ("456", "John Doe")]);
        let message = problems(resolve_against(&available, &[want_name("John Doe")]));
        assert!(message.contains("2 trainees share that name"), "{message}");
    }

    /// The worker rejects an unlisted id too, so the CLI must agree with it.
    #[test]
    fn an_unknown_id_is_reported() {
        let available = portal(&[("123", "John Doe")]);
        let message = problems(resolve_against(&available, &[want_id("999")]));
        assert!(message.contains("no trainee with that id"), "{message}");
    }

    /// The gap this closes: the same person named twice, once by id and once by
    /// name, must produce ONE submission rather than a duplicate record.
    #[test]
    fn the_same_person_by_id_and_by_name_dedupes_to_one() {
        let available = portal(&[("123", "John Doe"), ("456", "Jane Roe")]);
        let resolved =
            resolve_against(&available, &[want_id("123"), want_name("john doe")]).unwrap();
        assert_eq!(ids(&resolved), vec!["123"]);
    }

    #[test]
    fn dedupe_ignores_spacing_and_case_differences() {
        let available = portal(&[("123", "John Doe")]);
        let resolved =
            resolve_against(&available, &[want_name("john doe"), want_name("John  DOE")]).unwrap();
        assert_eq!(ids(&resolved), vec!["123"]);
    }

    #[test]
    fn distinct_trainees_are_all_kept_in_order() {
        let available = portal(&[("123", "John Doe"), ("456", "Jane Roe")]);
        let resolved = resolve_against(
            &available,
            &[want_id("123"), want_name("jane roe"), want_id("456")],
        )
        .unwrap();
        assert_eq!(ids(&resolved), vec!["123", "456"]);
    }

    /// An explicit name match wins over a differently-cased id key, and an id
    /// takes precedence when both are present on one entry.
    #[test]
    fn an_id_on_the_entry_wins_over_its_name() {
        let available = portal(&[("123", "John Doe"), ("456", "Jane Roe")]);
        let both = TraineeRef {
            id: Some("456".to_string()),
            name: Some("John Doe".to_string()),
        };
        let resolved = resolve_against(&available, &[both]).unwrap();
        assert_eq!(ids(&resolved), vec!["456"]);
    }

    #[test]
    fn every_unresolvable_entry_is_reported_in_one_pass() {
        let available = portal(&[("123", "John Doe")]);
        let message = problems(resolve_against(
            &available,
            &[want_name("Nobody"), want_id("999")],
        ));
        assert!(message.contains("Nobody"), "{message}");
        assert!(message.contains("999"), "{message}");
    }

    #[test]
    fn a_resolvable_batch_reports_no_problems() {
        let available = portal(&[("123", "John Doe")]);
        assert!(resolve_against(&available, &[want_id("123")]).is_ok());
    }
}
