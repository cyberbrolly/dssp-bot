//! Executes the shipped binary, always with an explicit offline worker fixture.
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BATCH: &str = include_str!("fixtures/batch.json");
const UNFINISHED: &str = include_str!("fixtures/unfinished.json");
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dssp-cli-fixture-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dssp-bot-core"));
        command
            .current_dir(&self.0)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.0)
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env(
                "DSSP_WORKER_PY",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/worker.py"),
            )
            .env("DSSP_FIXTURE_TRACE", self.0.join("trace.jsonl"))
            .env("DSSP_FIXTURE_STARTS", self.0.join("starts.txt"))
            .env("DSSP_FIXTURE_EVENTS", self.0.join("events.txt"))
            .env("DSSP_FIXTURE_SUBMISSIONS", self.0.join("submissions.jsonl"))
            .env("DSSP_CHECKPOINT", self.0.join("checkpoint.json"))
            .env("DSSP_PROFILE_DIR", self.0.join("profile"))
            .env_remove("DSSP_RESUME")
            .env_remove("DSSP_FIXTURE_DELAY");
        command
    }
    fn trace(&self) -> Vec<Value> {
        fs::read_to_string(self.0.join("trace.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process(Child);
impl Process {
    fn finish(&mut self) -> std::process::Output {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if self.0.try_wait().unwrap().is_some() {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = self.0.stdout.take() {
                    pipe.read_to_end(&mut stdout).unwrap();
                }
                if let Some(mut pipe) = self.0.stderr.take() {
                    pipe.read_to_end(&mut stderr).unwrap();
                }
                return std::process::Output {
                    status: self.0.wait().unwrap(),
                    stdout,
                    stderr,
                };
            }
            assert!(Instant::now() < deadline, "fixture process timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn http(port: u16, token: &str, method: &str, path: &str, body: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_secs(3),
    )
    .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nOrigin: chrome-extension://fixture-extension\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (header, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(!header.to_ascii_lowercase().contains("access-control-allow"));
    (
        header.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    )
}
fn start_daemon(scratch: &Scratch, resume_env: bool) -> (Process, u16, String) {
    let mut cmd = scratch.command();
    if resume_env {
        cmd.env("DSSP_RESUME", "1");
    }
    start_daemon_command(scratch, cmd)
}
fn start_daemon_command(scratch: &Scratch, mut cmd: Command) -> (Process, u16, String) {
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    cmd.arg("--daemon")
        .env("DSSP_DAEMON_PORT", port.to_string())
        .env(
            "DSSP_DAEMON_ORIGINS",
            "chrome-extension://fixture-extension",
        )
        .stderr(Stdio::from(
            fs::File::create(scratch.0.join("daemon.log")).unwrap(),
        ))
        .stdout(Stdio::null());
    drop(reservation);
    let mut process = Process(cmd.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok() {
            break;
        }
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "daemon exited: {}",
            fs::read_to_string(scratch.0.join("daemon.log")).unwrap()
        );
        assert!(Instant::now() < deadline, "daemon startup timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    let token = fs::read_to_string(scratch.0.join("dssp.daemon.token")).unwrap();
    (process, port, token)
}
fn complete(port: u16, token: &str, terminal: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = http(port, token, "GET", "/status", "").1;
        if status["state"] == terminal {
            return status;
        }
        assert!(Instant::now() < deadline, "completion timeout: {status}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn daemon_ignores_resume_environment_and_uses_explicit_request_acknowledgement() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("checkpoint.json"), UNFINISHED).unwrap();
    let (_process, port, token) = start_daemon(&scratch, true);
    assert_eq!(http(port, &token, "POST", "/batch", BATCH).0, 409);
    assert!(
        scratch.trace().is_empty(),
        "guard must precede worker handshake/ping"
    );
    let mut omitted: Value = serde_json::from_str(BATCH).unwrap();
    omitted.as_object_mut().unwrap().remove("resume");
    assert_eq!(
        http(port, &token, "POST", "/batch", &omitted.to_string()).0,
        409
    );
    assert!(scratch.trace().is_empty());
    assert!(
        fs::read_to_string(scratch.0.join("daemon.log"))
            .unwrap()
            .contains("DSSP_RESUME is ignored")
    );
    omitted["resume"] = json!(true);
    assert_eq!(
        http(port, &token, "POST", "/batch", &omitted.to_string()).0,
        202
    );
    let status = complete(port, &token, "stopped");
    assert_eq!(status["indeterminate"], 1);
    assert_eq!(status["successful"], 2);
    let submitted: Vec<_> = scratch
        .trace()
        .into_iter()
        .filter(|r| r["op"] == "submit_training")
        .collect();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0]["trainee"]["id"], "fixture-2");
}

#[test]
fn daemon_binary_executes_synthetic_worker_and_keeps_status_available() {
    let scratch = Scratch::new();
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    let mut command = scratch.command();
    command
        .arg("--daemon")
        .env("DSSP_DAEMON_PORT", port.to_string())
        .env(
            "DSSP_DAEMON_ORIGINS",
            "chrome-extension://fixture-extension",
        )
        .env("DSSP_FIXTURE_DELAY", "0.5")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    drop(reservation);
    let mut process = Process(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err() {
        assert!(process.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let token = fs::read_to_string(scratch.0.join("dssp.daemon.token")).unwrap();
    assert_eq!(http(port, &token, "GET", "/form-options", "").0, 200);
    assert_eq!(http(port, &token, "POST", "/batch", BATCH).0, 202);
    assert_eq!(http(port, &token, "GET", "/status", "").0, 200);
    assert_eq!(http(port, &token, "POST", "/batch", BATCH).0, 503);
    assert_eq!(complete(port, &token, "complete")["successful"], 2);
    assert_eq!(http(port, &token, "GET", "/report", "").1["total"], 2);
    assert_eq!(
        http(port, &token, "GET", "/checkpoint", "").1["status"],
        "finished"
    );
    let trace = scratch.trace();
    assert!(
        trace.iter().any(|r| r["op"] == "ensure_session"),
        "real WorkerClient handshake and request/response path"
    );
    assert_eq!(
        trace
            .iter()
            .filter(|r| r["op"] == "submit_training")
            .count(),
        2
    );
}

#[test]
fn cli_single_and_batch_remain_compatible_with_fixture_worker() {
    for batch in [false, true] {
        let scratch = Scratch::new();
        let mut input: Value = serde_json::from_str(BATCH).unwrap();
        input.as_object_mut().unwrap().remove("resume");
        if !batch {
            input.as_object_mut().unwrap().remove("trainees");
            input["trainee"] = json!({"id":"fixture-1"});
        }
        let path = scratch.0.join("fixture-input.json");
        fs::write(&path, input.to_string()).unwrap();
        let mut command = scratch.command();
        command
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = Process(command.spawn().unwrap()).finish();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        if batch {
            let report: Value = serde_json::from_str(&stdout).unwrap();
            assert_eq!(report["successful"], 2);
            assert_eq!(report["total"], 2);
            assert!(scratch.0.join("checkpoint.json").exists());
        } else {
            assert!(stdout.contains("RESULT: confirmed"), "{stdout}");
            assert!(!scratch.0.join("checkpoint.json").exists());
        }
        assert_eq!(
            scratch
                .trace()
                .iter()
                .filter(|r| r["op"] == "submit_training")
                .count(),
            if batch { 2 } else { 1 }
        );
    }
}

#[test]
fn cli_checkpoint_refusal_retains_exit_3_and_zero_worker_starts() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("checkpoint.json"), UNFINISHED).unwrap();
    let input = scratch.0.join("fixture-input.json");
    fs::write(&input, BATCH).unwrap();
    let mut command = scratch.command();
    command
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = Process(command.spawn().unwrap()).finish();
    assert_eq!(output.status.code(), Some(3));
    assert!(scratch.trace().is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refus"));
}

fn terminal_status(port: u16, token: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = http(port, token, "GET", "/status", "").1;
        if status["state"] == "stopped" || status["state"] == "complete" {
            return status;
        }
        assert!(Instant::now() < deadline, "completion timeout: {status}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn worker_starts(scratch: &Scratch) -> usize {
    fs::read_to_string(scratch.0.join("starts.txt"))
        .unwrap_or_default()
        .lines()
        .count()
}
fn submissions(scratch: &Scratch) -> Vec<Value> {
    scratch
        .trace()
        .into_iter()
        .filter(|r| r["op"] == "submit_training")
        .collect()
}
#[test]
fn stage32_startup_timeout_sends_no_requests() {
    assert_startup_failure("hang", "ready handshake timed out after 1000 ms");
}

#[test]
fn stage32_actual_submit_timeout_halts_without_retry() {
    assert_submit_halt(
        json!({"submit":"hang"}),
        "indeterminate",
        Some("NETWORK"),
        "worker response timed out after 500 ms",
    );
}

#[test]
fn stage32_late_submit_reply_does_not_change_timeout_or_send_again() {
    assert_submit_halt(
        json!({"submit":"delayed_reply", "reply_delay":0.8}),
        "indeterminate",
        Some("NETWORK"),
        "worker response timed out after 500 ms",
    );
}

#[test]
fn stage32_submit_timeout_and_eof_ignoring_shutdown_release_admission() {
    assert_submit_halt(
        json!({"submit":"hang", "shutdown":"hang"}),
        "indeterminate",
        Some("NETWORK"),
        "worker response timed out after 500 ms",
    );
}

#[test]
fn stage32_shutdown_timeout_preserves_confirmed_cli_result() {
    let scratch = Scratch::new();
    let path = scratch.0.join("fixture-input.json");
    fs::write(&path, BATCH).unwrap();
    let mut command = scratch.command();
    command
        .arg(path)
        .env(
            "DSSP_FIXTURE_SCENARIO",
            json!({"shutdown":"hang"}).to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    short_supervision(&mut command);
    let started = Instant::now();
    let output = Process(command.spawn().unwrap()).finish();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["successful"], 2);
    assert_eq!(report["aborted"], false);
    assert_eq!(submissions(&scratch).len(), 2);
    assert_eq!(worker_starts(&scratch), 1);
    assert!(
        fs::read_to_string(scratch.0.join("events.txt"))
            .unwrap()
            .contains("stdin_eof")
    );
}

#[test]
fn stage32_invalid_supervision_timeouts_refuse_before_worker_spawn() {
    for variable in [
        "DSSP_WORKER_READY_TIMEOUT_MS",
        "DSSP_WORKER_RESPONSE_TIMEOUT_MS",
        "DSSP_WORKER_SHUTDOWN_TIMEOUT_MS",
    ] {
        for value in ["0", "-1", "not-a-duration", "1.5", "18446744073709551616"] {
            let scratch = Scratch::new();
            let mut command = scratch.command();
            command.env(variable, value);
            let (_process, port, token) = start_daemon_command(&scratch, command);
            let reply = http(port, &token, "POST", "/batch", BATCH);
            assert_eq!(reply.0, 502);
            let error = reply.1["error"].as_str().unwrap();
            assert!(
                error.contains(variable) && error.contains("positive integer"),
                "{reply:?}"
            );
            assert_eq!(worker_starts(&scratch), 0);
            assert!(scratch.trace().is_empty());
            assert!(!scratch.0.join("checkpoint.json").exists());
            let path = scratch.0.join("fixture-input.json");
            fs::write(&path, BATCH).unwrap();
            let mut command = scratch.command();
            command
                .arg(path)
                .env(variable, value)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let output = Process(command.spawn().unwrap()).finish();
            assert_eq!(output.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&output.stderr).contains(variable));
            assert_eq!(worker_starts(&scratch), 0);
            assert!(scratch.trace().is_empty());
        }
    }
}

fn short_supervision(command: &mut Command) {
    command
        .env("DSSP_WORKER_READY_TIMEOUT_MS", "1000")
        .env("DSSP_WORKER_RESPONSE_TIMEOUT_MS", "500")
        .env("DSSP_WORKER_SHUTDOWN_TIMEOUT_MS", "1000");
}
fn assert_submit_halt(scenario: Value, outcome: &str, code: Option<&str>, message: &str) {
    let scratch = Scratch::new();
    let mut command = scratch.command();
    command.env("DSSP_FIXTURE_SCENARIO", scenario.to_string());
    let timed = matches!(scenario["submit"].as_str(), Some("hang" | "delayed_reply"));
    if timed {
        short_supervision(&mut command);
    }
    let (process, port, token) = start_daemon_command(&scratch, command);
    assert_eq!(http(port, &token, "POST", "/batch", BATCH).0, 202);
    if timed {
        let deadline = Instant::now() + Duration::from_secs(2);
        while submissions(&scratch).is_empty() {
            assert!(Instant::now() < deadline, "fixture never received submit");
            std::thread::sleep(Duration::from_millis(10));
        }
        // Poll the actual daemon while WorkerClient is awaiting a silent worker.
        let status = http(port, &token, "GET", "/status", "");
        assert_eq!(status.0, 200);
        assert_eq!(status.1["processed"], 0);
        let report = http(port, &token, "GET", "/report", "");
        assert_eq!(report, (200, Value::Null));
        let checkpoint = http(port, &token, "GET", "/checkpoint", "");
        assert_eq!(checkpoint.0, 200);
        assert_eq!(checkpoint.1["inFlight"], "fixture-1");
    }
    let status = terminal_status(port, &token);
    assert_eq!(status["state"], "stopped", "scenario {scenario}: {status}");
    let report = http(port, &token, "GET", "/report", "").1;
    assert_eq!(report["aborted"], true, "{scenario}: {report}");
    assert_eq!(report["total"], 2);
    assert_eq!(report["successful"], 0);
    assert_eq!(report["failed"], usize::from(outcome == "failed"));
    assert_eq!(
        report["indeterminate"],
        usize::from(outcome == "indeterminate")
    );
    assert_eq!(report["skipped"], 1);
    assert_eq!(status["failed"], report["failed"]);
    assert_eq!(status["indeterminate"], report["indeterminate"]);
    assert_eq!(status["skipped"], report["skipped"]);
    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["traineeId"], "fixture-1");
    assert_eq!(results[0]["outcome"], outcome);
    assert_eq!(results[0]["attempts"], 1);
    assert_eq!(results[0]["errorCode"], json!(code));
    assert!(
        results[0]["message"].as_str().unwrap().contains(message),
        "{results:?}"
    );
    assert_eq!(results[1]["traineeId"], "fixture-2");
    assert_eq!(results[1]["outcome"], "skipped");
    assert_eq!(results[1]["attempts"], 0);
    let sent = submissions(&scratch);
    assert_eq!(
        sent.len(),
        1,
        "exactly one submit, no retries/later trainees"
    );
    assert_eq!(sent[0]["trainee"]["id"], "fixture-1");
    assert_eq!(worker_starts(&scratch), 1);

    let snapshots: Vec<Value> = fs::read_to_string(scratch.0.join("submissions.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0]["request"], sent[0]);
    assert_eq!(snapshots[0]["checkpoint"]["in_flight"], "fixture-1");
    assert_eq!(snapshots[0]["checkpoint"]["pending"], json!(["fixture-2"]));
    assert_eq!(snapshots[0]["checkpoint"]["results"], json!([]));
    let saved = fs::read(scratch.0.join("checkpoint.json")).unwrap();
    let checkpoint: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(checkpoint["status"], "aborted");
    assert_eq!(checkpoint["total"], 2);
    assert_eq!(checkpoint["in_flight"], Value::Null);
    assert_eq!(checkpoint["pending"], json!([]));
    assert_eq!(checkpoint["started_at"], report["startedAt"]);
    assert_eq!(
        checkpoint["results"].as_array().unwrap().len(),
        results.len()
    );
    for (persisted, reported) in checkpoint["results"]
        .as_array()
        .unwrap()
        .iter()
        .zip(results)
    {
        assert_eq!(persisted["trainee_id"], reported["traineeId"]);
        assert_eq!(persisted["trainee_name"], reported["traineeName"]);
        assert_eq!(persisted["outcome"], reported["outcome"]);
        assert_eq!(persisted["attempts"], reported["attempts"]);
        assert_eq!(persisted["error_code"], reported["errorCode"]);
        assert_eq!(persisted["message"], reported["message"]);
    }
    // Terminal progress can precede teardown; wait for admission to be released.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut teardown_busy = false;
    loop {
        let reply = http(port, &token, "POST", "/batch", BATCH);
        if reply.0 != 503 {
            assert_eq!(reply.0, 409, "fresh admission must refuse: {reply:?}");
            break;
        }
        teardown_busy = true;
        assert!(Instant::now() < deadline, "fixture teardown timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    if scenario["shutdown"] == "hang" {
        assert!(
            teardown_busy,
            "report/status must be published while teardown still owns admission"
        );
    }
    assert_eq!(
        scratch
            .trace()
            .iter()
            .filter(|r| r["op"] == "submit_training")
            .count(),
        1
    );
    assert_eq!(worker_starts(&scratch), 1);
    assert_eq!(fs::read(scratch.0.join("checkpoint.json")).unwrap(), saved);
    if scenario["submit"] == "delayed_reply" {
        assert!(
            fs::read_to_string(scratch.0.join("events.txt"))
                .unwrap()
                .contains("late_reply")
        );
    }
    drop(process);
    let (_restarted, port, token) = start_daemon(&scratch, false);
    assert_eq!(http(port, &token, "POST", "/batch", BATCH).0, 409);
    assert_eq!(
        worker_starts(&scratch),
        1,
        "restart guard precedes new worker"
    );
    assert_eq!(submissions(&scratch), sent);
    assert_eq!(fs::read(scratch.0.join("checkpoint.json")).unwrap(), saved);

    let cli = Scratch::new();
    let input = cli.0.join("fixture-input.json");
    fs::write(&input, BATCH).unwrap();
    let run = || {
        let mut command = cli.command();
        command
            .arg(&input)
            .env("DSSP_FIXTURE_SCENARIO", scenario.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if timed {
            short_supervision(&mut command);
        }
        Process(command.spawn().unwrap()).finish()
    };
    let output = run();
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["aborted"], true);
    assert_eq!(report["total"], 2);
    assert_eq!(report["results"][0]["trainee_id"], "fixture-1");
    assert_eq!(report["results"][0]["outcome"], outcome);
    assert_eq!(report["results"][0]["error_code"], json!(code));
    assert_eq!(report["results"][0]["attempts"], 1);
    assert!(
        report["results"][0]["message"]
            .as_str()
            .unwrap()
            .contains(message)
    );
    assert_eq!(report["results"][1]["outcome"], "skipped");
    assert_eq!(report["results"][1]["attempts"], 0);
    assert_eq!(submissions(&cli).len(), 1);
    assert_eq!(worker_starts(&cli), 1);
    let saved = fs::read(cli.0.join("checkpoint.json")).unwrap();
    let checkpoint: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(checkpoint["status"], "aborted");
    assert_eq!(checkpoint["results"], report["results"]);
    let refused = run();
    assert_eq!(refused.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("refusing to start"));
    assert_eq!(worker_starts(&cli), 1);
    assert_eq!(submissions(&cli).len(), 1);
    assert_eq!(fs::read(cli.0.join("checkpoint.json")).unwrap(), saved);
}

#[test]
fn stage32_submit_wrong_op_halts_and_preserves_provenance() {
    assert_submit_halt(
        json!({"submit":"wrong_op"}),
        "indeterminate",
        Some("NETWORK"),
        "op mismatch",
    );
}

#[test]
fn stage32_submit_wrong_explicit_trainee_id_retains_requested_provenance() {
    assert_submit_halt(
        json!({"submit":"wrong_trainee_id"}),
        "indeterminate",
        Some("NETWORK"),
        "trainee_id mismatch",
    );
}

#[test]
fn stage32_optional_resolved_trainee_and_name_selector_remain_compatible() {
    for (selector, scenario) in [
        (
            json!({"id":"fixture-1"}),
            json!({"submit":"missing_trainee"}),
        ),
        (json!({"name":"Fixture One"}), json!({})),
    ] {
        let scratch = Scratch::new();
        let mut input: Value = serde_json::from_str(BATCH).unwrap();
        input.as_object_mut().unwrap().remove("trainees");
        input.as_object_mut().unwrap().remove("resume");
        input["trainee"] = selector.clone();
        let path = scratch.0.join("fixture-input.json");
        fs::write(&path, input.to_string()).unwrap();
        let mut command = scratch.command();
        command
            .arg(path)
            .env("DSSP_FIXTURE_SCENARIO", scenario.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = Process(command.spawn().unwrap()).finish();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("RESULT: confirmed"));
        let sent = submissions(&scratch);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["trainee"], selector);
        assert_eq!(worker_starts(&scratch), 1);
        assert!(!scratch.0.join("checkpoint.json").exists());
    }
}

macro_rules! submit_fault {
    ($name:ident, $mode:literal, $code:expr, $message:literal) => {
        #[test]
        fn $name() {
            assert_submit_halt(json!({"submit":$mode}), "indeterminate", $code, $message);
        }
    };
}
submit_fault!(
    stage32_submit_malformed_json,
    "malformed_json",
    Some("NETWORK"),
    "invalid protocol line"
);
submit_fault!(
    stage32_submit_missing_version,
    "missing_version",
    Some("NETWORK"),
    "missing field"
);
submit_fault!(
    stage32_submit_wrong_version,
    "wrong_version",
    Some("NETWORK"),
    "protocol version mismatch"
);
submit_fault!(
    stage32_submit_wrong_job_id,
    "wrong_job_id",
    Some("NETWORK"),
    "job_id mismatch"
);
submit_fault!(
    stage32_submit_missing_op,
    "missing_op",
    Some("NETWORK"),
    "op mismatch"
);
submit_fault!(
    stage32_submit_missing_outcome,
    "missing_outcome",
    None,
    "unrecognized outcome"
);
submit_fault!(
    stage32_submit_unknown_outcome,
    "unknown_outcome",
    None,
    "unrecognized outcome"
);
submit_fault!(
    stage32_submit_wrong_status,
    "wrong_status",
    Some("NETWORK"),
    "invalid protocol line"
);
submit_fault!(
    stage32_submit_ready_echo,
    "ready_echo",
    None,
    "unexpected ready"
);
submit_fault!(
    stage32_submit_bare_ready,
    "bare_ready",
    Some("NETWORK"),
    "job_id mismatch"
);
submit_fault!(
    stage32_submit_early_eof,
    "early_eof",
    Some("NETWORK"),
    "worker closed stdout"
);

fn assert_startup_failure(mode: &str, message: &str) {
    let scratch = Scratch::new();
    let mut command = scratch.command();
    command.env("DSSP_FIXTURE_SCENARIO", json!({"startup":mode}).to_string());
    short_supervision(&mut command);
    let (_process, port, token) = start_daemon_command(&scratch, command);
    for expected_starts in 1..=2 {
        let reply = http(port, &token, "POST", "/batch", BATCH);
        assert_eq!(reply.0, 502, "{mode}: {reply:?}");
        assert!(
            reply.1["error"].as_str().unwrap().contains(message),
            "{reply:?}"
        );
        assert_eq!(worker_starts(&scratch), expected_starts);
        assert!(scratch.trace().is_empty(), "no request before valid ready");
        assert!(!scratch.0.join("checkpoint.json").exists());
        assert_eq!(http(port, &token, "GET", "/report", "").1, Value::Null);
        assert_eq!(http(port, &token, "GET", "/status", "").0, 200);
    }
    let cli = Scratch::new();
    let input = cli.0.join("fixture-input.json");
    fs::write(&input, BATCH).unwrap();
    let mut command = cli.command();
    command
        .arg(input)
        .env("DSSP_FIXTURE_SCENARIO", json!({"startup":mode}).to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    short_supervision(&mut command);
    let output = Process(command.spawn().unwrap()).finish();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains(message));
    assert_eq!(worker_starts(&cli), 1);
    assert!(cli.trace().is_empty());
    assert!(!cli.0.join("checkpoint.json").exists());
}
macro_rules! startup_fault {
    ($name:ident, $mode:literal, $message:literal) => {
        #[test]
        fn $name() {
            assert_startup_failure($mode, $message);
        }
    };
}
startup_fault!(
    stage32_startup_invalid_ready,
    "invalid_ready",
    "expected ready handshake"
);
startup_fault!(
    stage32_startup_malformed_json,
    "malformed_json",
    "invalid protocol line"
);
startup_fault!(
    stage32_startup_missing_version,
    "missing_version",
    "missing field"
);
startup_fault!(
    stage32_startup_wrong_version,
    "wrong_version",
    "protocol version mismatch"
);
startup_fault!(
    stage32_startup_early_eof,
    "early_eof",
    "worker closed stdout"
);

#[test]
fn stage32_spawn_missing_and_nonexecutable_paths_fail_without_worker() {
    use std::os::unix::fs::PermissionsExt;
    for exists in [false, true] {
        let scratch = Scratch::new();
        let interpreter = scratch.0.join("not-an-interpreter");
        let message = if exists {
            "failed to spawn worker"
        } else {
            "worker interpreter not found"
        };
        if exists {
            fs::write(&interpreter, "offline non-executable fixture").unwrap();
            fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut command = scratch.command();
        command.env("DSSP_WORKER_PY", &interpreter);
        let (_process, port, token) = start_daemon_command(&scratch, command);
        for _ in 0..2 {
            let reply = http(port, &token, "POST", "/batch", BATCH);
            assert_eq!(reply.0, 502);
            assert!(
                reply.1["error"].as_str().unwrap().contains(message),
                "{reply:?}"
            );
        }
        assert_eq!(worker_starts(&scratch), 0);
        assert!(scratch.trace().is_empty());
        assert!(!scratch.0.join("checkpoint.json").exists());
        let input = scratch.0.join("fixture-input.json");
        fs::write(&input, BATCH).unwrap();
        let mut command = scratch.command();
        command
            .arg(input)
            .env("DSSP_WORKER_PY", interpreter)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = Process(command.spawn().unwrap()).finish();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
        assert_eq!(worker_starts(&scratch), 0);
    }
}

#[test]
fn stage32_explicit_errors_without_delivery_proof_never_retry() {
    for code in [
        "SESSION_EXPIRED",
        "PORTAL_STRUCTURE_CHANGED",
        "ELEMENT_NOT_FOUND",
        "NETWORK",
        "TIMEOUT",
        "PORTAL_UNAVAILABLE",
        "CONFIRMATION_UNKNOWN",
        "UNKNOWN_FIXTURE_CODE",
    ] {
        for proof in [None, Some(false)] {
            let mut scenario = json!({"submit":"error", "error_code":code});
            if let Some(proof) = proof {
                scenario["proof"] = json!(proof);
            }
            assert_submit_halt(
                scenario,
                "indeterminate",
                Some(code),
                "submission may have landed",
            );
        }
    }
}
#[test]
fn stage32_expiry_and_structure_halt_even_when_provably_unsent() {
    for code in ["SESSION_EXPIRED", "PORTAL_STRUCTURE_CHANGED"] {
        assert_submit_halt(
            json!({"submit":"error", "error_code":code, "proof":true}),
            "failed",
            Some(code),
            "synthetic worker error",
        );
    }
}

#[test]
fn stage32_safe_transient_preflight_retries_are_bounded() {
    for code in [
        "ELEMENT_NOT_FOUND",
        "NETWORK",
        "TIMEOUT",
        "PORTAL_UNAVAILABLE",
    ] {
        for failures in [1, 10] {
            let scratch = Scratch::new();
            let mut command = scratch.command();
            command.env(
                "DSSP_FIXTURE_SCENARIO",
                json!({"submit":"error", "error_code":code, "proof":true, "failures":failures})
                    .to_string(),
            );
            let (_process, port, token) = start_daemon_command(&scratch, command);
            assert_eq!(http(port, &token, "POST", "/batch", BATCH).0, 202);
            assert_eq!(terminal_status(port, &token)["state"], "complete");
            let report = http(port, &token, "GET", "/report", "").1;
            let exhausted = failures == 10;
            assert_eq!(report["total"], 2);
            assert_eq!(report["aborted"], false);
            assert_eq!(report["indeterminate"], 0);
            assert_eq!(report["skipped"], 0);
            assert_eq!(report["failed"], if exhausted { 2 } else { 0 });
            assert_eq!(report["successful"], if exhausted { 0 } else { 2 });
            assert_eq!(
                report["results"][0]["attempts"],
                if exhausted { 3 } else { 2 }
            );
            assert_eq!(
                report["results"][1]["attempts"],
                if exhausted { 3 } else { 1 }
            );
            if exhausted {
                for result in report["results"].as_array().unwrap() {
                    assert_eq!(result["errorCode"], code);
                    assert!(
                        result["message"]
                            .as_str()
                            .unwrap()
                            .contains("gave up after 3 attempts")
                    );
                }
            }
            let sent = submissions(&scratch);
            assert_eq!(sent.len(), if exhausted { 6 } else { 3 });
            let first_count = if exhausted { 3 } else { 2 };
            assert!(
                sent[..first_count]
                    .iter()
                    .all(|r| r["trainee"]["id"] == "fixture-1" && r["job_id"] == sent[0]["job_id"])
            );
            assert!(
                sent[first_count..]
                    .iter()
                    .all(|r| r["trainee"]["id"] == "fixture-2"
                        && r["job_id"] == sent[first_count]["job_id"])
            );
            assert_ne!(sent[0]["job_id"], sent[first_count]["job_id"]);
            assert_eq!(worker_starts(&scratch), 1);
            let checkpoint: Value =
                serde_json::from_slice(&fs::read(scratch.0.join("checkpoint.json")).unwrap())
                    .unwrap();
            assert_eq!(checkpoint["status"], "finished");
            assert_eq!(
                checkpoint["results"][0]["attempts"],
                report["results"][0]["attempts"]
            );
            assert_eq!(
                checkpoint["results"][1]["attempts"],
                report["results"][1]["attempts"]
            );
        }
    }
}

#[test]
fn stage32_rejection_continues_and_counts_without_retry() {
    for scenario in [
        json!({"submit":"rejected"}),
        json!({"submit":"error", "error_code":"UNKNOWN_FIXTURE_CODE", "proof":true}),
    ] {
        let scratch = Scratch::new();
        let mut command = scratch.command();
        command.env("DSSP_FIXTURE_SCENARIO", scenario.to_string());
        let (_process, port, token) = start_daemon_command(&scratch, command);
        assert_eq!(http(port, &token, "POST", "/batch", BATCH).0, 202);
        assert_eq!(terminal_status(port, &token)["state"], "complete");
        let report = http(port, &token, "GET", "/report", "").1;
        assert_eq!(report["total"], 2);
        assert_eq!(report["successful"], 1);
        assert_eq!(report["failed"], 1);
        assert_eq!(report["skipped"], 0);
        assert_eq!(report["indeterminate"], 0);
        assert_eq!(report["aborted"], false);
        assert_eq!(report["results"][0]["outcome"], "failed");
        assert_eq!(report["results"][1]["outcome"], "success");
        assert_eq!(report["results"][0]["attempts"], 1);
        assert_eq!(report["results"][1]["attempts"], 1);
        let sent = submissions(&scratch);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["trainee"]["id"], "fixture-1");
        assert_eq!(sent[1]["trainee"]["id"], "fixture-2");
        assert_eq!(worker_starts(&scratch), 1);
        assert_eq!(
            http(port, &token, "GET", "/checkpoint", "").1["status"],
            "finished"
        );
    }
}
