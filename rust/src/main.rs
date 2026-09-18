//! DSSP-Bot coordinator — first milestone: run one trainee end to end.
//!
//! Flow: spawn worker → ready → ensure_session (manual login on first run) →
//! get_form_options (informational) → submit_training with Rust-owned retry →
//! print one classified result line.
//!
//! The protocol layer is intentionally complete ahead of full use; unused
//! members are expected at this milestone.
#![allow(dead_code)]

mod decision;
mod engine;
mod protocol;
mod queue;
mod report;
mod state;
mod worker;

use std::fs;
use std::process::exit;

use decision::{decide_submit, Decision, RetryPolicy};
use protocol::{Request, SessionInput, Status, TraineeRef};
use serde::Deserialize;
use worker::WorkerClient;

#[derive(Deserialize)]
struct Job {
    trainee: TraineeRef,
    session: SessionInput,
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
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

    let mut client = match WorkerClient::spawn() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("dssp-bot: {e}");
            exit(1);
        }
    };
    if let Err(e) = client.wait_ready() {
        eprintln!("dssp-bot: worker never became ready: {e}");
        exit(1);
    }

    // 1. Session gate. On the first run this blocks while you log in manually
    //    in the opened browser window.
    eprintln!("dssp-bot: ensuring portal session…");
    match client.send(&Request::ensure_session(new_id())) {
        Ok(r) if r.status == Status::Ok => eprintln!("dssp-bot: session ok"),
        Ok(r) => {
            eprintln!("dssp-bot: session failed: {}", r.summary());
            exit(1);
        }
        Err(e) => {
            eprintln!("dssp-bot: session error: {e}");
            exit(1);
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
            Request::submit_training(job_id.clone(), job.trainee.clone(), job.session.clone());

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
    exit(code);
}
