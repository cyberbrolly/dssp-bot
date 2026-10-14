//! Supervisor for the Python worker: spawn, handshake, request/response.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::protocol::{Request, Response, Status};

#[derive(Clone, Copy)]
struct Timeouts {
    ready: Duration,
    response: Duration,
    shutdown: Duration,
}
impl Timeouts {
    fn environment() -> Result<Self, String> {
        Ok(Self {
            ready: environment_timeout("DSSP_WORKER_READY_TIMEOUT_MS", 10_000)?,
            // ensure_session may wait up to five minutes for manual login.
            response: environment_timeout("DSSP_WORKER_RESPONSE_TIMEOUT_MS", 360_000)?,
            shutdown: environment_timeout("DSSP_WORKER_SHUTDOWN_TIMEOUT_MS", 2_000)?,
        })
    }
}
fn environment_timeout(name: &str, default_ms: u64) -> Result<Duration, String> {
    match std::env::var(name) {
        Ok(value) => parse_timeout(name, &value),
        Err(std::env::VarError::NotPresent) => Ok(Duration::from_millis(default_ms)),
        Err(_) => Err(format!(
            "{name} must be a positive integer number of milliseconds"
        )),
    }
}
fn parse_timeout(name: &str, value: &str) -> Result<Duration, String> {
    let invalid = || format!("{name} must be a positive integer number of milliseconds");
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let ms = value.parse::<u64>().map_err(|_| invalid())?;
    let duration = Duration::from_millis(ms);
    if ms == 0 || Instant::now().checked_add(duration).is_none() {
        return Err(invalid());
    }
    Ok(duration)
}

pub struct WorkerClient {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Option<Receiver<Result<String, String>>>,
    readers: Vec<JoinHandle<()>>,
    timeouts: Timeouts,
    unusable: Option<String>,
    reaped: bool,
}

impl WorkerClient {
    /// Spawn `python/.venv/bin/python -m app.worker` from the repo root.
    ///
    /// `DSSP_WORKER_PY` overrides the interpreter path (tests use this to
    /// point at a scratch venv).
    pub fn spawn() -> Result<Self, String> {
        // Reject configuration before creating any worker process.
        let timeouts = Timeouts::environment()?;
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest.parent().ok_or("cannot resolve repo root")?;
        let python_dir = repo_root.join("python");
        let python = std::env::var("DSSP_WORKER_PY")
            .map(PathBuf::from)
            .unwrap_or_else(|_| python_dir.join(".venv").join("bin").join("python"));

        if !python.exists() {
            return Err(format!(
                "worker interpreter not found at {} — create python/.venv first",
                python.display()
            ));
        }

        let mut command = Command::new(&python);
        command.arg("-m").arg("app.worker").current_dir(&python_dir);
        Self::spawn_command(command, timeouts)
            .map_err(|e| format!("failed to spawn worker {}: {e}", python.display()))
    }

    fn spawn_command(mut command: Command, timeouts: Timeouts) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().expect("piped worker stdin");
        let stdout = child.stdout.take().expect("piped worker stdout");
        let stderr = child.stderr.take().expect("piped worker stderr");
        let (sender, responses) = mpsc::channel();
        let stdout_reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                let result = match reader.read_line(&mut line) {
                    Ok(0) => Err("worker closed stdout".to_string()),
                    Ok(_) => Ok(line),
                    Err(e) => Err(format!("read from worker failed: {e}")),
                };
                let terminal = result.is_err();
                if sender.send(result).is_err() || terminal {
                    break;
                }
            }
        });
        let stderr_reader = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[worker] {line}");
            }
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            responses: Some(responses),
            readers: vec![stdout_reader, stderr_reader],
            timeouts,
            unusable: None,
            reaped: false,
        })
    }

    /// Wait for the startup handshake line `{"v":1,"status":"ready"}`.
    pub fn wait_ready(&mut self) -> Result<(), String> {
        self.check_usable()?;
        let result = self
            .read_response(self.timeouts.ready, "ready handshake")
            .and_then(|resp| match resp.status {
                Status::Ready => Ok(()),
                other => Err(format!("expected ready handshake, got {other:?}")),
            });
        if let Err(error) = &result {
            self.invalidate(error);
        }
        result
    }

    /// Send one request and read its response. Enforces the `job_id` and `op` echoes.
    pub fn send(&mut self, req: &Request) -> Result<Response, String> {
        self.check_usable()?;
        let result = self.send_request(req);
        match &result {
            Err(error) => self.invalidate(error),
            Ok(resp) if resp.status == Status::Ready => {
                self.invalidate("unexpected ready line mid-flow")
            }
            _ => {}
        }
        result
    }

    fn send_request(&mut self, req: &Request) -> Result<Response, String> {
        let line = serde_json::to_string(req).map_err(|e| format!("serialize: {e}"))?;

        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "worker stdin closed".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("write to worker failed: {e}"))?;

        let resp = self.read_response(self.timeouts.response, "worker response")?;

        if resp.job_id != req.job_id {
            return Err(format!(
                "job_id mismatch: sent {}, received {} — protocol violation",
                req.job_id, resp.job_id
            ));
        }

        if resp.op != req.op {
            return Err(format!(
                "op mismatch: sent {}, received {} — protocol violation",
                req.op, resp.op
            ));
        }

        // Only compare a returned identity with an explicitly requested ID.
        // Name selectors may resolve aliases, and error replies may omit trainee.
        if let (Some(expected), Some(actual)) = (
            req.trainee.as_ref().and_then(|t| t.id.as_deref()),
            resp.trainee.as_ref(),
        ) && expected != actual.id
        {
            return Err(format!(
                "trainee_id mismatch: sent {}, received {} — protocol violation",
                expected, actual.id
            ));
        }

        Ok(resp)
    }

    fn check_usable(&self) -> Result<(), String> {
        match &self.unusable {
            Some(reason) => Err(format!("worker client unusable: {reason}")),
            None => Ok(()),
        }
    }

    fn invalidate(&mut self, reason: &str) {
        self.unusable.get_or_insert_with(|| reason.to_string());
        self.stdin.take();
        self.responses.take(); // Late replies cannot be consumed by another request.
    }

    fn read_response(&self, timeout: Duration, phase: &str) -> Result<Response, String> {
        let line = self
            .responses
            .as_ref()
            .ok_or("worker response channel closed")?
            .recv_timeout(timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => {
                    format!("{phase} timed out after {} ms", timeout.as_millis())
                }
                RecvTimeoutError::Disconnected => "worker response reader disconnected".to_string(),
            })??;
        let resp: Response = serde_json::from_str(line.trim())
            .map_err(|e| format!("invalid protocol line ({e}): {:?}", line.trim()))?;

        check_version(&resp)?;

        Ok(resp)
    }

    /// Close stdin, allow a bounded EOF grace period, then kill and reap the worker.
    pub fn shutdown(&mut self) {
        self.invalidate("worker shut down");
        if self.reaped {
            return;
        }
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.reaped = true;
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("dssp-bot: checking worker exit failed: {error}");
                    break;
                }
            }
            let remaining = self.timeouts.shutdown.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
        match self.child.kill() {
            Ok(()) => match self.child.wait() {
                Ok(_) => self.reaped = true,
                Err(error) => eprintln!("dssp-bot: reaping killed worker failed: {error}"),
            },
            Err(error) => {
                // Do not follow a failed kill with an unbounded wait on a live child.
                self.reaped = matches!(self.child.try_wait(), Ok(Some(_)));
                if !self.reaped {
                    eprintln!("dssp-bot: killing worker failed: {error}");
                }
            }
        }
        // Direct-child pipe readers finish on EOF after exit/kill. Never join here:
        // a descendant retaining a pipe must not turn shutdown back into a hang.
    }
}

/// Refuse a response written to a different version of the protocol.
///
/// `protocol::V` was sent on every request and never read back off a response, so
/// a worker that disagreed about what the fields *mean* parsed exactly like one that
/// agreed. That matters most for `proves_nothing_submitted`: the recovery gate
/// reduces to Rust reading that flag correctly, and a worker speaking v2 is free to
/// redefine it while every field still deserializes — the one disagreement this
/// protocol has no other way to notice.
///
/// Both directions are refused. An older worker reporting its own version to a newer
/// build is the same disagreement from the other side, and silence about a field the
/// reader expects is not agreement.
///
/// Mid-submit this becomes a transport error, which the engine already treats as
/// `Indeterminate`: the worker replied, so the request was processed and the
/// submission may have landed. That is the safe bucket — reported, never replayed.
fn check_version(resp: &Response) -> Result<(), String> {
    if resp.v == crate::protocol::V {
        return Ok(());
    }

    Err(format!(
        "protocol version mismatch: the worker speaks v{}, this build speaks v{} — refusing to \
         act on fields that may not mean the same thing",
        resp.v,
        crate::protocol::V
    ))
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::V;

    fn response(v: u32) -> Response {
        serde_json::from_str(&format!(
            r#"{{"v":{v},"status":"error","error_code":"PORTAL_UNAVAILABLE",
                 "proves_nothing_submitted":false}}"#
        ))
        .expect("the fixture parses")
    }

    #[test]
    fn a_response_at_this_builds_version_is_accepted() {
        assert!(check_version(&response(V)).is_ok());
    }

    #[test]
    fn a_response_from_a_different_protocol_version_is_refused() {
        let error = check_version(&response(V + 1)).expect_err("a version this build cannot read");

        assert!(error.contains("protocol version mismatch"), "{error}");
        assert!(
            error.contains("speaks v2"),
            "names the worker's version: {error}"
        );
        assert!(error.contains("speaks v1"), "and this build's: {error}");
    }

    /// The check is a comparison, not a guess, because `v` is the one field with no
    /// `#[serde(default)]`: a response that omits it fails to parse at all rather
    /// than defaulting to something this build would accept.
    #[test]
    fn a_response_that_omits_the_version_does_not_parse() {
        assert!(serde_json::from_str::<Response>(r#"{"status":"ok"}"#).is_err());
    }

    #[test]
    fn timeout_values_require_positive_integer_milliseconds() {
        for invalid in [
            "",
            "0",
            "000",
            "-1",
            "+1",
            "1.5",
            "NaN",
            " 1",
            "1 ",
            "18446744073709551616",
        ] {
            let error = parse_timeout("fixture timeout", invalid).unwrap_err();
            assert!(error.contains("positive integer"), "{invalid}: {error}");
        }
        assert_eq!(
            parse_timeout("fixture", "1").unwrap(),
            Duration::from_millis(1)
        );
        assert_eq!(
            parse_timeout("fixture", "360000").unwrap(),
            Duration::from_secs(360)
        );
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("dssp-worker-timeout-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn worker(&self, scenario: serde_json::Value) -> WorkerClient {
            let mut command = Command::new(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/worker.py"),
            );
            command
                .current_dir(&self.0)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &self.0)
                .env("PYTHONNOUSERSITE", "1")
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .env("DSSP_PROFILE_DIR", self.0.join("profile"))
                .env("DSSP_CHECKPOINT", self.0.join("checkpoint.json"))
                .env("DSSP_FIXTURE_TRACE", self.0.join("trace.jsonl"))
                .env("DSSP_FIXTURE_EVENTS", self.0.join("events.txt"))
                .env("DSSP_FIXTURE_SCENARIO", scenario.to_string());
            WorkerClient::spawn_command(
                command,
                Timeouts {
                    ready: Duration::from_secs(2),
                    response: Duration::from_millis(100),
                    shutdown: Duration::from_millis(100),
                },
            )
            .unwrap()
        }
        fn trace(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(self.0.join("trace.jsonl"))
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn submit() -> Request {
        Request::submit_training(
            "fixture-job",
            crate::protocol::TraineeRef {
                id: Some("fixture-1".into()),
                name: None,
            },
            crate::protocol::SessionInput {
                training_date: "2026-10-01".into(),
                instructor: "Fixture Instructor".into(),
                training_type: "Practical".into(),
            },
        )
    }
    fn readers_finish(worker: &mut WorkerClient) {
        let started = Instant::now();
        while !worker.readers.iter().all(JoinHandle::is_finished) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "worker readers did not finish after child exit"
            );
            thread::sleep(Duration::from_millis(10));
        }
        for reader in worker.readers.drain(..) {
            reader.join().unwrap();
        }
    }

    #[test]
    fn timeout_poison_discards_late_reply_and_never_writes_again() {
        let fixture = Fixture::new();
        let mut worker =
            fixture.worker(serde_json::json!({"submit":"delayed_reply", "reply_delay":0.4}));
        worker.wait_ready().unwrap();
        let error = worker.send(&submit()).unwrap_err();
        assert!(
            error.contains("worker response timed out after 100 ms"),
            "{error}"
        );
        assert!(
            worker
                .send(&Request::ping("after-timeout"))
                .unwrap_err()
                .contains("unusable")
        );
        let started = Instant::now();
        while worker.child.try_wait().unwrap().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "late-reply fixture did not exit"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            std::fs::read_to_string(fixture.0.join("events.txt"))
                .unwrap()
                .contains("late_reply")
        );
        assert!(
            worker
                .send(&Request::ping("after-late-reply"))
                .unwrap_err()
                .contains("unusable")
        );
        assert!(worker.wait_ready().unwrap_err().contains("unusable"));
        worker.shutdown();
        readers_finish(&mut worker);
        assert_eq!(fixture.trace().len(), 1);
        assert_eq!(fixture.trace()[0]["op"], "submit_training");
    }

    #[test]
    fn protocol_and_transport_failures_poison_before_another_write() {
        for mode in [
            "malformed_json",
            "wrong_version",
            "wrong_job_id",
            "wrong_op",
            "wrong_trainee_id",
            "ready_echo",
            "early_eof",
        ] {
            let fixture = Fixture::new();
            let mut worker = fixture.worker(serde_json::json!({"submit":mode}));
            worker.wait_ready().unwrap();
            let result = worker.send(&submit());
            if mode == "ready_echo" {
                assert_eq!(result.unwrap().status, Status::Ready);
            } else {
                assert!(result.is_err(), "{mode}");
            }
            assert!(
                worker
                    .send(&Request::ping("never-written"))
                    .unwrap_err()
                    .contains("unusable")
            );
            worker.shutdown();
            readers_finish(&mut worker);
            assert_eq!(fixture.trace().len(), 1, "{mode}");
        }
    }

    #[test]
    fn ready_timeout_poison_sends_nothing_and_shutdown_reaps() {
        let fixture = Fixture::new();
        let mut worker = fixture.worker(serde_json::json!({"startup":"hang", "shutdown":"hang"}));
        worker.timeouts.ready = Duration::from_millis(100);
        let error = worker.wait_ready().unwrap_err();
        assert!(
            error.contains("ready handshake timed out after 100 ms"),
            "{error}"
        );
        assert!(
            worker
                .send(&Request::ping("never-written"))
                .unwrap_err()
                .contains("unusable")
        );
        worker.shutdown();
        assert!(worker.reaped);
        readers_finish(&mut worker);
        assert!(fixture.trace().is_empty());
    }

    #[test]
    fn shutdown_kills_eof_ignoring_worker_reaps_and_finishes_readers() {
        use std::os::unix::process::ExitStatusExt;
        let fixture = Fixture::new();
        let mut worker = fixture.worker(serde_json::json!({"shutdown":"hang"}));
        worker.wait_ready().unwrap();
        worker.send(&Request::ping("fixture-ping")).unwrap();
        let started = Instant::now();
        worker.shutdown();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(worker.child.try_wait().unwrap().unwrap().signal(), Some(9));
        assert!(worker.reaped);
        assert!(
            std::fs::read_to_string(fixture.0.join("events.txt"))
                .unwrap()
                .contains("stdin_eof")
        );
        assert!(
            worker
                .send(&Request::ping("after-shutdown"))
                .unwrap_err()
                .contains("unusable")
        );
        readers_finish(&mut worker);
        worker.shutdown();
        assert_eq!(fixture.trace().len(), 1);
    }
}
