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
            .env(
                "DSSP_WORKER_PY",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/worker.py"),
            )
            .env("DSSP_FIXTURE_TRACE", self.0.join("trace.jsonl"))
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
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
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
    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = reservation.local_addr().unwrap().port();
    let mut cmd = scratch.command();
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
    if resume_env {
        cmd.env("DSSP_RESUME", "1");
    }
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
