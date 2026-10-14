//! Integration tests through real TCP sockets, with fixture-only transports.
use super::*;
use std::net::{Shutdown, SocketAddr};
use std::sync::Condvar;
use std::sync::atomic::AtomicBool;

const BATCH: &str = include_str!("../tests/fixtures/batch.json");
const UNFINISHED: &str = include_str!("../tests/fixtures/unfinished.json");
const RESPONSES: &str = include_str!("../tests/fixtures/responses.json");
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ORIGIN: &str = "chrome-extension://fixture-extension";

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("dssp-daemon-{}", crate::new_id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn checkpoint(&self) -> PathBuf {
        self.0.join("checkpoint.json")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Probe {
    starts: AtomicUsize,
    calls: Mutex<Vec<String>>,
    held: Mutex<bool>,
    release: Condvar,
    outcome: Mutex<Option<String>>,
}
impl Probe {
    fn unblock(&self) {
        *self.held.lock().unwrap() = false;
        self.release.notify_all();
    }
    fn submissions(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|s| s.strip_prefix("submit_training:").map(str::to_owned))
            .collect()
    }
}
struct FixtureWorker(Arc<Probe>);
impl Transport for FixtureWorker {
    fn send(&mut self, request: &Request) -> Result<Response, String> {
        self.0.calls.lock().unwrap().push(format!(
            "{}:{}",
            request.op,
            request
                .trainee
                .as_ref()
                .and_then(|t| t.id.as_deref())
                .unwrap_or("")
        ));
        if request.op == crate::protocol::op::SUBMIT_TRAINING {
            let mut held = self.0.held.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(60);
            while *held {
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .expect("fixture release deadline");
                held = self.0.release.wait_timeout(held, left).unwrap().0;
            }
        }
        let fixtures: Value = serde_json::from_str(RESPONSES).unwrap();
        let mut response = fixtures[request.op].clone();
        response["job_id"] = json!(request.job_id);
        if request.op == crate::protocol::op::SUBMIT_TRAINING {
            response["trainee"] = json!({"id": request.trainee.as_ref().unwrap().id,
                "name": if request.trainee.as_ref().unwrap().id.as_deref() == Some("fixture-1") { "Fixture One" } else { "Fixture Two" }});
            if let Some(outcome) = self.0.outcome.lock().unwrap().as_ref() {
                response["outcome"] = json!(outcome);
            }
        }
        serde_json::from_value(response).map_err(|e| e.to_string())
    }
}

struct Server {
    address: SocketAddr,
    daemon: Arc<Daemon>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    probe: Arc<Probe>,
    active: Arc<AtomicUsize>,
    scratch: Scratch,
}
impl Server {
    fn new(checkpoint: Option<&str>, held: bool) -> Self {
        Self::with_factory(checkpoint, held, None)
    }
    fn with_factory(checkpoint: Option<&str>, held: bool, factory: Option<Factory>) -> Self {
        let scratch = Scratch::new();
        if let Some(cp) = checkpoint {
            fs::write(scratch.checkpoint(), cp).unwrap();
        }
        let probe = Arc::new(Probe::default());
        *probe.held.lock().unwrap() = held;
        let p = probe.clone();
        let daemon = Arc::new(Daemon::new(
            Config {
                port: 0,
                origins: vec![ORIGIN.into(), "moz-extension://fixture-uuid".into()],
                token: TOKEN.into(),
                checkpoint: scratch.checkpoint(),
            },
            factory.unwrap_or_else(|| {
                Arc::new(move || {
                    p.starts.fetch_add(1, Ordering::SeqCst);
                    Ok(Box::new(FixtureWorker(p.clone())))
                })
            }),
        ));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let d = daemon.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let count = active.clone();
        let thread = std::thread::spawn(move || {
            let mut clients = Vec::new();
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(client) = d.dispatch(stream.unwrap(), &count).unwrap() {
                    clients.push(client);
                }
            }
            for client in clients {
                let _ = client.join(); // Panic-injection tests must still clean up.
            }
        });
        Self {
            address,
            daemon,
            stop,
            thread: Some(thread),
            probe,
            active,
            scratch,
        }
    }
    fn raw(&self, request: &str) -> (u16, Value, String) {
        raw(self.address, request)
    }
    fn request(&self, method: &str, path: &str, body: &str) -> (u16, Value, String) {
        self.raw(&format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {TOKEN}\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", self.address, body.len()))
    }
    fn wait_for(&self, condition: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (code, value, _) = self.request("GET", "/status", "");
            assert_eq!(code, 200);
            if condition(&value) {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "status never reached condition: {value}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.daemon.control(false); // Also release a paused fixture on unwind.
        self.probe.unblock();
        let deadline = Instant::now() + Duration::from_secs(6);
        while self.daemon.state.lock().unwrap().busy && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn raw(address: SocketAddr, request: &str) -> (u16, Value, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(6)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    response(stream)
}
fn response(mut stream: TcpStream) -> (u16, Value, String) {
    let mut bytes = String::new();
    // Closing an early-rejected request with unread bytes can reset TCP.
    // A reset is acceptable only after a complete framed response arrived.
    if let Err(e) = stream.read_to_string(&mut bytes) {
        assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset, "{e}");
    }
    let (headers, body) = bytes.split_once("\r\n\r\n").expect("HTTP response");
    let code = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(
        !headers
            .to_ascii_lowercase()
            .contains("access-control-allow")
    );
    let length: usize = headers
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(length, body.len());
    (code, serde_json::from_str(body).unwrap(), headers.into())
}

#[test]
fn authenticated_reads_and_exact_extension_origins() {
    let server = Server::new(None, false);
    assert!(server.address.ip().is_loopback());
    let status = server.request("GET", "/status", "").1;
    assert_eq!(status, progress("idle", 0, &[], None));
    assert_eq!(server.request("GET", "/report", "").1, Value::Null);
    assert_eq!(server.request("GET", "/checkpoint", "").1, Value::Null);
    let base =
        format!("GET /status HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n");
    assert_eq!(server.raw(&format!("{base}\r\n")).0, 200);
    assert_eq!(
        server
            .raw(&format!(
                "{base}Origin: moz-extension://fixture-uuid\r\n\r\n"
            ))
            .0,
        200
    );
    for origin in [
        "https://attacker.example",
        "null",
        "chrome-extension://fixture-extension.evil",
        "chrome-extension://fixture-extension/",
        "chrome-extension://another",
    ] {
        assert_eq!(
            server.raw(&format!("{base}Origin: {origin}\r\n\r\n")).0,
            403,
            "{origin}"
        );
    }
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn unauthorized_and_simple_content_types_have_no_effect() {
    let server = Server::new(None, false);
    for auth in [
        "",
        "Authorization: Bearer wrong\r\n",
        "Authorization: Basic wrong\r\n",
    ] {
        assert_eq!(server.raw(&format!("POST /batch HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{BATCH}", BATCH.len())).0, 401);
    }
    for ct in [
        "",
        "Content-Type: text/plain\r\n",
        "Content-Type: application/x-www-form-urlencoded\r\n",
        "Content-Type: multipart/form-data\r\n",
    ] {
        assert_eq!(server.raw(&format!("POST /batch HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n{ct}Content-Length: {}\r\n\r\n{BATCH}", BATCH.len())).0, 415);
    }
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    assert!(!server.scratch.checkpoint().exists());
}

#[test]
fn strict_parser_rejects_duplicates_smuggling_and_invalid_lines() {
    let server = Server::new(None, false);
    let head = format!(
        "POST /batch HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n"
    );
    for extra in [
        "Authorization: Bearer wrong\r\n",
        "ORIGIN: x\r\nOrigin: y\r\n",
        "Content-Length: 0\r\ncontent-length: 1\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Transfer-Encoding: chunked\r\nContent-Length: 0\r\n",
        "Expect: 100-continue\r\n",
        "Content-Length: +1\r\n",
        "Content-Length: -1\r\n",
        "Content-Length: 1,1\r\n",
        " folded: x\r\n",
        "Header : x\r\n",
        "Header: x\u{7f}\r\n",
        "Header: x\nInjected: y\r\n",
        "Host: other\r\n",
    ] {
        assert_eq!(
            server.raw(&format!("{head}{extra}\r\n")).0,
            400,
            "{extra:?}"
        );
    }
    for line in [
        "GET /status HTTP/1.0",
        "GET  /status HTTP/1.1",
        "GET http://localhost/status HTTP/1.1",
        "G\tET /status HTTP/1.1",
        "GET /sta\ttus HTTP/1.1",
        "GET /status#fragment HTTP/1.1",
    ] {
        assert_eq!(
            server.raw(&format!("{line}\r\nHost: localhost\r\n\r\n")).0,
            400,
            "{line:?}"
        );
    }
    assert_eq!(server.raw("GET /status HTTP/1.1\r\n\r\n").0, 400);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn body_and_header_caps_and_truncation_are_enforced() {
    let server = Server::new(None, false);
    assert_eq!(
        server
            .raw("POST /batch HTTP/1.1\r\nHost: localhost\r\nContent-Length: 65537\r\n\r\n")
            .0,
        413
    );
    assert_eq!(server.raw("POST /batch HTTP/1.1\r\nHost: localhost\r\nContent-Length: 999999999999999999999999\r\n\r\n").0, 413);
    assert_eq!(
        server
            .raw("POST /batch HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\nx")
            .0,
        400
    );
    assert_eq!(
        server
            .raw(&format!(
                "GET /status HTTP/1.1\r\nX-Long: {}\r\n\r\n",
                "a".repeat(HEADER_LIMIT)
            ))
            .0,
        413
    );
    let body = " ".repeat(BODY_LIMIT);
    assert_eq!(server.request("POST", "/batch", &body).0, 422);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn slow_request_times_out_without_blocking_status() {
    let server = Server::new(None, false);
    let mut slow = TcpStream::connect(server.address).unwrap();
    slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    slow.write_all(b"GET /status HTTP/1.1\r\nHost:").unwrap();
    let began = Instant::now();
    assert_eq!(server.request("GET", "/status", "").0, 200);
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(response(slow).0, 408);
}

#[test]
fn pause_pending_settles_once_then_resume_keeps_the_same_worker_and_queue() {
    let server = Server::new(None, true);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| server.probe.submissions() == ["fixture-1"]);
    for _ in 0..2 {
        let (code, value, _) = server.request("POST", "/batch/pause", "{}");
        assert_eq!(code, 200);
        assert_eq!(value["state"], "submitting");
        assert_eq!(value["pauseRequested"], true);
        assert_eq!(value["processed"], 0);
        assert_eq!(value["currentTraineeName"], "Fixture One");
    }
    let active = server.request("GET", "/checkpoint", "").1;
    assert_eq!(active["status"], "running");
    assert_eq!(active["inFlight"], "fixture-1");
    assert_eq!(active["pending"], json!(["fixture-2"]));
    let began = Instant::now();
    for _ in 0..3 {
        assert_eq!(
            server.request("GET", "/status", "").1["pauseRequested"],
            true
        );
    }
    assert!(began.elapsed() < Duration::from_secs(1));
    server.probe.unblock();
    let paused = server.wait_for(|v| v["state"] == "paused");
    assert_eq!(paused["pauseRequested"], true);
    assert_eq!(paused["processed"], 1);
    assert_eq!(paused["successful"], 1);
    assert_eq!(paused["remaining"], 1);
    assert!(paused.get("currentTraineeName").is_none());
    let checkpoint = server.request("GET", "/checkpoint", "").1;
    assert_eq!(checkpoint["status"], "paused");
    assert_eq!(checkpoint["inFlight"], Value::Null);
    assert_eq!(checkpoint["pending"], json!(["fixture-2"]));
    assert_eq!(checkpoint["results"].as_array().unwrap().len(), 1);
    assert_eq!(checkpoint["results"][0]["traineeId"], "fixture-1");
    assert_eq!(checkpoint["startedAt"], active["startedAt"]);
    for _ in 0..3 {
        assert_eq!(server.request("POST", "/batch/pause", "{}").1, paused);
        assert_eq!(server.request("GET", "/status", "").1, paused);
        assert_eq!(server.request("GET", "/checkpoint", "").1, checkpoint);
        assert_eq!(server.probe.submissions(), ["fixture-1"]);
    }
    assert_eq!(server.request("POST", "/batch", BATCH).0, 503);
    assert_eq!(server.request("GET", "/form-options", "").0, 503);
    assert_eq!(server.request("GET", "/report", "").1, Value::Null);
    assert_eq!(server.request("POST", "/batch/stop", "{}").0, 405);
    let (code, resumed, _) = server.request("POST", "/batch/resume", "{}");
    assert_eq!(code, 200);
    assert_eq!(resumed["pauseRequested"], false);
    let finished = server.wait_for(|v| v["state"] == "complete");
    assert_eq!(finished["successful"], 2);
    assert_eq!(finished["pauseRequested"], false);
    assert_eq!(server.probe.submissions(), ["fixture-1", "fixture-2"]);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.request("GET", "/checkpoint", "").1["startedAt"],
        active["startedAt"]
    );
    wait_until(|| !server.daemon.state.lock().unwrap().busy);
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(server.request("POST", path, "{}").0, 409);
    }
    assert_eq!(server.request("GET", "/status", "").1, finished);
    assert!(server.daemon.state.lock().unwrap().control.is_none());

    // A later admission has a fresh controller, not the predecessor's intent.
    *server.probe.held.lock().unwrap() = true;
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| server.probe.submissions().len() == 3);
    assert_eq!(
        server.request("GET", "/status", "").1["pauseRequested"],
        false
    );
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 2);
    server.probe.unblock();
    server.wait_for(|v| v["state"] == "complete");
    assert_eq!(
        server.probe.submissions(),
        ["fixture-1", "fixture-2", "fixture-1", "fixture-2"]
    );
}

#[test]
fn resume_is_idempotent_while_active_and_cancels_a_pending_pause() {
    let server = Server::new(None, true);
    let idle = server.request("GET", "/status", "").1;
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(server.request("POST", path, "{}").0, 409);
    }
    assert_eq!(server.request("GET", "/status", "").1, idle);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| server.probe.submissions() == ["fixture-1"]);
    for _ in 0..2 {
        let reply = server.request("POST", "/batch/resume", "{}");
        assert_eq!(reply.0, 200);
        assert_eq!(reply.1["pauseRequested"], false);
        assert_eq!(reply.1["state"], "submitting");
    }
    assert_eq!(
        server.request("POST", "/batch/pause", "{}").1["pauseRequested"],
        true
    );
    for _ in 0..2 {
        assert_eq!(
            server.request("POST", "/batch/resume", "{}").1["pauseRequested"],
            false
        );
    }
    assert_eq!(server.probe.submissions(), ["fixture-1"]);
    server.probe.unblock();
    server.wait_for(|v| v["state"] == "complete");
    assert_eq!(server.probe.submissions(), ["fixture-1", "fixture-2"]);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 1);
}

#[test]
fn indeterminate_after_pause_still_aborts_without_retrying_or_pausing() {
    let server = Server::new(None, true);
    *server.probe.outcome.lock().unwrap() = Some("indeterminate".into());
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| server.probe.submissions() == ["fixture-1"]);
    assert_eq!(server.request("POST", "/batch/pause", "{}").0, 200);
    server.probe.unblock();
    let status = server.wait_for(|v| v["state"] == "stopped" && v["processed"] == 2);
    assert_eq!(status["indeterminate"], 1);
    assert_eq!(status["skipped"], 1);
    assert_eq!(status["pauseRequested"], false);
    assert_eq!(server.probe.submissions(), ["fixture-1"]);
    assert_eq!(
        server.request("GET", "/checkpoint", "").1["status"],
        "aborted"
    );
    wait_until(|| !server.daemon.state.lock().unwrap().busy);
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(server.request("POST", path, "{}").0, 409);
    }
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
}

#[test]
fn paused_restart_requires_explicit_batch_recovery_with_zero_workers_on_refusal() {
    let original = Server::new(None, true);
    assert_eq!(original.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| original.probe.submissions() == ["fixture-1"]);
    assert_eq!(original.request("POST", "/batch/pause", "{}").0, 200);
    original.probe.unblock();
    original.wait_for(|v| v["state"] == "paused");
    let saved = fs::read_to_string(original.scratch.checkpoint()).unwrap();
    let restarted = Server::new(Some(&saved), false);
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(restarted.request("POST", path, "{}").0, 409);
    }
    for _ in 0..2 {
        assert_eq!(restarted.request("POST", "/batch", BATCH).0, 409);
    }
    assert_eq!(restarted.probe.starts.load(Ordering::SeqCst), 0);
    assert_eq!(restarted.request("GET", "/status", "").1["state"], "idle");
    let cp = restarted.request("GET", "/checkpoint", "").1;
    assert_eq!(cp["status"], "interrupted");
    assert_eq!(cp["pending"], json!(["fixture-2"]));
    assert_eq!(cp["inFlight"], Value::Null);
    assert_eq!(cp["results"][0]["traineeId"], "fixture-1");
    let mut acknowledged: Value = serde_json::from_str(BATCH).unwrap();
    acknowledged["resume"] = json!(true);
    assert_eq!(
        restarted
            .request("POST", "/batch", &acknowledged.to_string())
            .0,
        202
    );
    restarted.wait_for(|v| v["state"] == "complete");
    assert_eq!(restarted.probe.submissions(), ["fixture-2"]);
    assert_eq!(restarted.probe.starts.load(Ordering::SeqCst), 1);
    assert_eq!(restarted.request("GET", "/report", "").1["successful"], 2);
    assert_eq!(original.probe.submissions(), ["fixture-1"]);
    assert_eq!(original.request("POST", "/batch/resume", "{}").0, 200);
    original.wait_for(|v| v["state"] == "complete");
}

#[test]
fn pause_resume_share_auth_origin_content_and_body_limits_without_effects() {
    let server = Server::new(None, true);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| server.probe.submissions() == ["fixture-1"]);
    let before = server.request("GET", "/status", "").1;
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(server.raw(&format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}")).0, 401);
        assert_eq!(server.raw(&format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nOrigin: https://foreign.example\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}")).0, 403);
        assert_eq!(server.raw(&format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n{{}}")).0, 415);
        assert_eq!(server.raw(&format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", BODY_LIMIT + 1)).0, 413);
        for body in [
            "",
            "null",
            "[]",
            "true",
            "{",
            "{}{}",
            "{\"pause\":true}",
            "{\"resume\":true}",
        ] {
            assert_eq!(server.request("POST", path, body).0, 422, "{body}");
        }
        assert_eq!(server.request("GET", path, "{}").0, 405);
    }
    assert_eq!(server.request("GET", "/status", "").1, before);
    assert_eq!(server.probe.submissions(), ["fixture-1"]);
    let reply = server.raw(&format!("POST /batch/pause HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: 4\r\n\r\n {{}} "));
    assert_eq!(reply.0, 200);
    assert_eq!(reply.1["pauseRequested"], true);
    assert_eq!(server.request("POST", "/batch/resume", "{}").0, 200);
    server.probe.unblock();
    server.wait_for(|v| v["state"] == "complete");
}

#[test]
fn pause_checkpoint_failure_never_publishes_paused_or_submits_the_next_trainee() {
    let server = Server::new(None, true);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| server.probe.submissions() == ["fixture-1"]);
    assert_eq!(server.request("POST", "/batch/pause", "{}").0, 200);
    fs::remove_file(server.scratch.checkpoint()).unwrap();
    fs::create_dir(server.scratch.checkpoint()).unwrap();
    server.probe.unblock();
    let stopped = server.wait_for(|v| v["state"] == "stopped");
    assert_eq!(stopped["pauseRequested"], false);
    wait_until(|| !server.daemon.state.lock().unwrap().busy);
    assert_eq!(server.probe.submissions(), ["fixture-1"]);
    assert_eq!(server.request("GET", "/report", "").1, Value::Null);
    assert!(server.daemon.state.lock().unwrap().control.is_none());
    assert_eq!(server.request("POST", "/batch/resume", "{}").0, 409);
}

#[test]
fn controls_during_form_options_are_409_without_state_change() {
    let probe = Arc::new(Probe::default());
    let admission = Gate::new();
    let p = probe.clone();
    let gate = admission.clone();
    let factory: Factory = Arc::new(move || {
        p.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ControlledWorker {
            inner: FixtureWorker(p.clone()),
            admission: Some(gate.clone()),
            teardown: None,
            fail_ensure: 0,
            ensures: 0,
        }))
    });
    let server = Server::with_factory(None, false, Some(factory));
    let _release = ReleaseOnDrop(admission.clone());
    let address = server.address;
    let read = std::thread::spawn(move || {
        raw(
            address,
            &format!(
                "GET /form-options HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
            ),
        )
    });
    wait_until(|| admission.arrived.load(Ordering::SeqCst));
    let before = server.request("GET", "/status", "").1;
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(server.request("POST", path, "{}").0, 409);
    }
    assert_eq!(server.request("GET", "/status", "").1, before);
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    assert!(probe.submissions().is_empty());
    admission.release();
    assert_eq!(read.join().unwrap().0, 200);
}

struct SettleFault {
    inner: FixtureWorker,
    panic: bool,
    teardown: Arc<Gate>,
}
impl Transport for SettleFault {
    fn send(&mut self, request: &Request) -> Result<Response, String> {
        let response = self.inner.send(request)?;
        if request.op == crate::protocol::op::SUBMIT_TRAINING {
            assert!(!self.panic, "synthetic panic after pending pause");
            return Err("synthetic unknown submission after pending pause".into());
        }
        Ok(response)
    }
}
impl Drop for SettleFault {
    fn drop(&mut self) {
        let _ = self.teardown.wait();
    }
}

#[test]
fn pending_pause_transport_error_and_panic_clear_controls_before_worker_teardown() {
    for panic in [false, true] {
        let probe = Arc::new(Probe::default());
        *probe.held.lock().unwrap() = true;
        let teardown = Gate::new();
        let p = probe.clone();
        let gate = teardown.clone();
        let factory: Factory = Arc::new(move || {
            p.starts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(SettleFault {
                inner: FixtureWorker(p.clone()),
                panic,
                teardown: gate.clone(),
            }))
        });
        let server = Server::with_factory(None, false, Some(factory));
        let _release = ReleaseOnDrop(teardown.clone());
        assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
        wait_until(|| probe.submissions() == ["fixture-1"]);
        assert_eq!(server.request("POST", "/batch/pause", "{}").0, 200);
        probe.unblock();
        wait_until(|| teardown.arrived.load(Ordering::SeqCst));
        let status = server.request("GET", "/status", "").1;
        assert_eq!(status["state"], "stopped");
        assert_eq!(status["pauseRequested"], false);
        assert!(status.get("currentTraineeName").is_none());
        for path in ["/batch/pause", "/batch/resume"] {
            assert_eq!(server.request("POST", path, "{}").0, 409);
        }
        assert_eq!(server.request("POST", "/batch", BATCH).0, 503);
        assert!(server.daemon.state.lock().unwrap().control.is_none());
        assert_eq!(probe.submissions(), ["fixture-1"]);
        let checkpoint = server.request("GET", "/checkpoint", "").1;
        if panic {
            assert_eq!(checkpoint["status"], "running");
            assert_eq!(checkpoint["inFlight"], "fixture-1");
        } else {
            assert_eq!(checkpoint["status"], "aborted");
            assert_eq!(status["indeterminate"], 1);
            assert_eq!(status["skipped"], 1);
        }
        teardown.release();
        wait_until(|| !server.daemon.state.lock().unwrap().busy);
        assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
        assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn resume_checkpoint_failure_stops_before_any_later_submission() {
    let server = Server::new(None, true);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| server.probe.submissions() == ["fixture-1"]);
    assert_eq!(server.request("POST", "/batch/pause", "{}").0, 200);
    server.probe.unblock();
    server.wait_for(|v| v["state"] == "paused");
    // Atomic checkpoint replacement cannot rename a file over this scratch directory.
    fs::remove_file(server.scratch.checkpoint()).unwrap();
    fs::create_dir(server.scratch.checkpoint()).unwrap();
    let reply = server.request("POST", "/batch/resume", "{}");
    assert_eq!(reply.0, 200);
    assert_eq!(reply.1["pauseRequested"], false);
    server.wait_for(|v| v["state"] == "stopped");
    wait_until(|| !server.daemon.state.lock().unwrap().busy);
    assert_eq!(server.probe.submissions(), ["fixture-1"]);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 1);
    assert!(server.daemon.state.lock().unwrap().control.is_none());
    assert_eq!(server.request("POST", "/batch/resume", "{}").0, 409);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 1);
}

#[test]
fn reserved_and_unknown_endpoints_remain_unsupported() {
    let server = Server::new(None, false);
    for (method, path) in [
        ("GET", "/batch/pause"),
        ("GET", "/batch/resume"),
        ("POST", "/batch/stop"),
        ("OPTIONS", "/batch"),
        ("GET", "/batch"),
        ("PUT", "/batch"),
        ("GET", "/unknown"),
    ] {
        assert_eq!(server.request(method, path, "{}").0, 405);
    }
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn checkpoint_refusal_matches_guard_and_starts_zero_workers() {
    let server = Server::new(Some(UNFINISHED), false);
    let (code, body, _) = server.request("POST", "/batch", BATCH);
    assert_eq!(code, 409);
    let expected =
        crate::recovery::guard(&CheckpointStore::new(server.scratch.checkpoint()), false)
            .err()
            .unwrap();
    assert_eq!(body["error"], expected);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    let checkpoint = server.request("GET", "/checkpoint", "").1;
    assert_eq!(checkpoint["status"], "interrupted");
    assert_eq!(checkpoint["inFlight"], "fixture-crash");
    assert_eq!(checkpoint["pending"], json!(["fixture-2"]));
    assert!(checkpoint.get("startedAt").is_some());
}

#[test]
fn corrupt_checkpoint_refuses_even_explicit_resume_before_factory() {
    let server = Server::new(Some("not JSON"), false);
    for resume in [false, true] {
        let mut batch: Value = serde_json::from_str(BATCH).unwrap();
        batch["resume"] = json!(resume);
        assert_eq!(server.request("POST", "/batch", &batch.to_string()).0, 409);
    }
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    assert_eq!(server.request("GET", "/checkpoint", "").0, 500);
}

#[test]
fn explicit_resume_submits_only_never_attempted_and_carries_crash_record() {
    let server = Server::new(Some(UNFINISHED), false);
    let mut batch: Value = serde_json::from_str(BATCH).unwrap();
    batch["resume"] = json!(true);
    assert_eq!(server.request("POST", "/batch", &batch.to_string()).0, 202);
    let status = server.wait_for(|v| v["state"] == "stopped" && v["processed"] == 3);
    assert_eq!(status["successful"], 2);
    assert_eq!(status["indeterminate"], 1);
    assert_eq!(server.probe.submissions(), ["fixture-2"]);
    let report = server.request("GET", "/report", "").1;
    assert_eq!(report["aborted"], true);
    assert_eq!(report["results"][1]["traineeId"], "fixture-crash");
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
}

#[test]
fn status_is_responsive_and_second_batch_is_503_during_submission() {
    let server = Server::new(None, true);
    let (code, accepted, _) = server.request("POST", "/batch", BATCH);
    assert_eq!(code, 202);
    assert!(uuid::Uuid::parse_str(accepted["batchId"].as_str().unwrap()).is_ok());
    let status = server.wait_for(|v| v["state"] == "submitting");
    assert_eq!(status["total"], 2);
    assert_eq!(status["currentTraineeName"], "Fixture One");
    assert_eq!(status["processed"], 0);
    let began = Instant::now();
    assert_eq!(server.request("POST", "/batch", BATCH).0, 503);
    assert_eq!(server.request("GET", "/form-options", "").0, 503);
    assert_eq!(server.request("GET", "/status", "").0, 200);
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        server.request("GET", "/checkpoint", "").1["inFlight"],
        "fixture-1"
    );
    server.probe.unblock();
    let status = server.wait_for(|v| v["state"] == "complete");
    assert_eq!(
        status,
        progress(
            "complete",
            2,
            &CheckpointStore::new(server.scratch.checkpoint())
                .load()
                .unwrap()
                .unwrap()
                .results,
            None
        )
    );
    assert_eq!(server.probe.submissions(), ["fixture-1", "fixture-2"]);
    let report = server.request("GET", "/report", "").1;
    assert_eq!(report["successful"], 2);
    assert_eq!(report["successRate"], 1.0);
    assert!(report.get("startedAt").is_some());
    assert_eq!(
        server.request("GET", "/checkpoint", "").1["status"],
        "finished"
    );
}

#[test]
fn form_options_are_shaped_for_extension() {
    let server = Server::new(None, false);
    let (code, options, _) = server.request("GET", "/form-options", "");
    assert_eq!(code, 200);
    assert_eq!(
        options["instructors"][0],
        json!({"value":"instructor-1","label":"Fixture Instructor"})
    );
    assert_eq!(options["trainingTypes"][0]["value"], "practical");
    assert!(server.probe.submissions().is_empty());
    assert_eq!(server.request("GET", "/status", "").1["state"], "idle");
}

#[test]
fn unresolvable_and_invalid_batches_submit_nothing() {
    let server = Server::new(None, false);
    for invalid in ["{}", "{", "{\"trainees\":[],\"session\":{}}"] {
        assert_eq!(server.request("POST", "/batch", invalid).0, 422);
    }
    for field in ["resume", "extra"] {
        let mut batch: Value = serde_json::from_str(BATCH).unwrap();
        batch[field] = json!("not a boolean");
        assert_eq!(server.request("POST", "/batch", &batch.to_string()).0, 422);
    }
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    let mut batch: Value = serde_json::from_str(BATCH).unwrap();
    batch["trainees"] = json!([{"id":"not-on-fixture-portal"}]);
    assert_eq!(server.request("POST", "/batch", &batch.to_string()).0, 422);
    assert!(server.probe.submissions().is_empty());
    assert!(!server.scratch.checkpoint().exists());
}

#[test]
fn indeterminate_aborts_without_replaying_and_reports_counts() {
    let server = Server::new(None, false);
    *server.probe.outcome.lock().unwrap() = Some("indeterminate".into());
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    let status = server.wait_for(|v| v["state"] == "stopped" && v["processed"] == 2);
    assert_eq!(status["indeterminate"], 1);
    assert_eq!(status["skipped"], 1);
    assert_eq!(server.probe.submissions(), ["fixture-1"]);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
}

#[test]
fn token_is_random_persisted_0600_and_unsafe_files_are_refused() {
    let scratch = Scratch::new();
    let path = scratch.0.join("token");
    let first = token_file(&path).unwrap();
    assert_eq!(first.len(), 64);
    assert_eq!(token_file(&path).unwrap(), first);
    assert_ne!(token_file(&scratch.0.join("other")).unwrap(), first);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(token_file(&path).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let symlink = scratch.0.join("link");
    std::os::unix::fs::symlink(&path, &symlink).unwrap();
    assert!(token_file(&symlink).is_err());
    let hardlink = scratch.0.join("hardlink");
    fs::hard_link(&path, hardlink).unwrap();
    assert!(token_file(&path).is_err());
    fs::write(scratch.0.join("bad"), "invalid").unwrap();
    fs::set_permissions(scratch.0.join("bad"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(token_file(&scratch.0.join("bad")).is_err());
}

#[test]
fn origin_configuration_has_no_wildcards_paths_or_web_origins() {
    for origin in [ORIGIN, "moz-extension://abc-123"] {
        assert!(valid_origin(origin));
    }
    for origin in [
        "https://example.com",
        "chrome-extension://*",
        "chrome-extension://",
        "moz-extension://uuid/path",
        "chrome-extension://id ",
        "chrome-extension://id:80",
    ] {
        assert!(!valid_origin(origin));
    }
}

#[test]
fn resume_without_predecessor_still_requires_a_nonempty_roster() {
    let server = Server::new(None, false);
    let mut batch: Value = serde_json::from_str(BATCH).unwrap();
    batch["resume"] = json!(true);
    batch["trainees"] = json!([]);
    assert_eq!(server.request("POST", "/batch", &batch.to_string()).0, 422);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn no_work_resume_closes_settled_predecessor_without_worker() {
    let mut checkpoint: Value = serde_json::from_str(UNFINISHED).unwrap();
    checkpoint["total"] = json!(1);
    checkpoint["pending"] = json!([]);
    checkpoint["in_flight"] = Value::Null;
    let server = Server::new(Some(&checkpoint.to_string()), false);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
    let mut batch: Value = serde_json::from_str(BATCH).unwrap();
    batch["resume"] = json!(true);
    assert_eq!(server.request("POST", "/batch", &batch.to_string()).0, 202);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    assert_eq!(server.request("GET", "/status", "").1["state"], "complete");
    assert_eq!(
        server.request("GET", "/checkpoint", "").1["status"],
        "finished"
    );
    assert_eq!(server.request("GET", "/report", "").1["startedAt"], "1000");
}

#[test]
fn failed_pre_submission_checkpoint_write_prevents_worker_submission() {
    let server = Server::new(None, false);
    // The slot is absent (guard accepts it), but its parent cannot be created
    // by the engine. This exercises the existing fatal pre-submit write gate.
    let config = Config {
        checkpoint: server.scratch.0.join("missing-parent/checkpoint.json"),
        ..server.daemon.config.clone()
    };
    let daemon = Arc::new(Daemon::new(config, server.daemon.factory.clone()));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let d = daemon.clone();
    let connection = std::thread::spawn(move || d.connection(listener.accept().unwrap().0));
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "POST /batch HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{BATCH}", BATCH.len()).unwrap();
    assert_eq!(response(stream).0, 202);
    connection.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while daemon.state.lock().unwrap().busy {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(server.probe.submissions().is_empty());
    let state = daemon.state.lock().unwrap();
    assert_eq!(state.progress["state"], "stopped");
    assert_eq!(state.report.as_ref().unwrap().skipped, 2);
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "fixture condition timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn batch_wire(address: SocketAddr, body: &str) -> String {
    format!(
        "POST /batch HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

struct Gate {
    held: Mutex<bool>,
    arrived: AtomicBool,
    changed: Condvar,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            held: Mutex::new(true),
            arrived: AtomicBool::new(false),
            changed: Condvar::new(),
        })
    }
    fn wait(&self) -> bool {
        let mut held = self.held.lock().unwrap();
        self.arrived.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(10);
        while *held {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            held = self.changed.wait_timeout(held, left).unwrap().0;
        }
        true
    }
    fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.changed.notify_all();
    }
}
struct ReleaseOnDrop(Arc<Gate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct ControlledWorker {
    inner: FixtureWorker,
    admission: Option<Arc<Gate>>,
    teardown: Option<Arc<Gate>>,
    fail_ensure: usize,
    ensures: usize,
}
impl Transport for ControlledWorker {
    fn send(&mut self, request: &Request) -> Result<Response, String> {
        if request.op == crate::protocol::op::ENSURE_SESSION {
            self.ensures += 1;
            if self.ensures == 1 && self.admission.as_ref().is_some_and(|g| !g.wait()) {
                return Err("admission fixture timeout".into());
            }
            if self.ensures == self.fail_ensure {
                return Err("synthetic session transport failure".into());
            }
        }
        self.inner.send(request)
    }
}
impl Drop for ControlledWorker {
    fn drop(&mut self) {
        if let Some(gate) = &self.teardown {
            let _ = gate.wait();
        }
    }
}

struct FeedOnDrop(Option<fs::File>);
impl Drop for FeedOnDrop {
    fn drop(&mut self) {
        if let Some(mut file) = self.0.take() {
            let _ = file.write_all(UNFINISHED.as_bytes());
        }
    }
}

#[test]
fn checkpoint_read_refusal_serializes_competing_admission_without_worker() {
    let server = Server::new(None, false);
    // A scratch FIFO pauses the actual recovery::guard read, not a mocked gate.
    assert!(
        std::process::Command::new("mkfifo")
            .arg(server.scratch.checkpoint())
            .status()
            .unwrap()
            .success()
    );
    let address = server.address;
    let first = std::thread::spawn(move || raw(address, &batch_wire(address, BATCH)));
    // Opening the writer proves guard is reading; withholding bytes pauses it.
    // Feed on unwind too, so a failed assertion cannot strand server teardown.
    let mut feed = FeedOnDrop(Some(
        fs::OpenOptions::new()
            .write(true)
            .open(server.scratch.checkpoint())
            .unwrap(),
    ));
    assert!(server.daemon.state.lock().unwrap().busy);
    assert_eq!(server.request("GET", "/status", "").1["state"], "idle");
    let mut acknowledged: Value = serde_json::from_str(BATCH).unwrap();
    acknowledged["resume"] = json!(true);
    assert_eq!(
        server
            .request("POST", "/batch", &acknowledged.to_string())
            .0,
        503
    );
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    {
        let mut writer = feed.0.take().unwrap();
        writer.write_all(UNFINISHED.as_bytes()).unwrap();
    }
    assert_eq!(first.join().unwrap().0, 409);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
    assert_eq!(
        server.request("GET", "/checkpoint", "").1["inFlight"],
        "fixture-crash"
    );
}

#[test]
fn simultaneous_admissions_start_exactly_one_worker() {
    let server = Server::new(None, true);
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let mut clients = Vec::new();
    for _ in 0..8 {
        let barrier = barrier.clone();
        let address = server.address;
        clients.push(std::thread::spawn(move || {
            barrier.wait();
            raw(address, &batch_wire(address, BATCH)).0
        }));
    }
    let codes: Vec<_> = clients.into_iter().map(|c| c.join().unwrap()).collect();
    assert_eq!(codes.iter().filter(|c| **c == 202).count(), 1);
    assert_eq!(codes.iter().filter(|c| **c == 503).count(), 7);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 1);
    server.probe.unblock();
    server.wait_for(|p| p["state"] == "complete");
}

#[test]
fn status_is_responsive_while_admission_waits_for_session() {
    let probe = Arc::new(Probe::default());
    let admission = Gate::new();
    let p = probe.clone();
    let gate = admission.clone();
    let factory: Factory = Arc::new(move || {
        p.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ControlledWorker {
            inner: FixtureWorker(p.clone()),
            admission: Some(gate.clone()),
            teardown: None,
            fail_ensure: 0,
            ensures: 0,
        }))
    });
    let server = Server::with_factory(None, false, Some(factory));
    let _release = ReleaseOnDrop(admission.clone());
    let address = server.address;
    let first = std::thread::spawn(move || raw(address, &batch_wire(address, BATCH)));
    wait_until(|| admission.arrived.load(Ordering::SeqCst));
    assert_eq!(
        server.request("GET", "/status", "").1["state"],
        "initializing"
    );
    assert_eq!(server.request("POST", "/batch", BATCH).0, 503);
    assert_eq!(server.request("GET", "/form-options", "").0, 503);
    assert!(probe.submissions().is_empty());
    let before = server.request("GET", "/status", "").1;
    for path in ["/batch/pause", "/batch/resume"] {
        let reply = server.request("POST", path, "{}");
        assert_eq!(reply.0, 409);
        assert!(reply.1["error"].as_str().unwrap().contains("admission"));
    }
    assert_eq!(server.request("GET", "/status", "").1, before);
    admission.release();
    assert_eq!(first.join().unwrap().0, 202);
    server.wait_for(|p| p["state"] == "complete");
}

#[test]
fn engine_error_stops_progress_but_holds_admission_until_worker_teardown() {
    let probe = Arc::new(Probe::default());
    let teardown = Gate::new();
    let p = probe.clone();
    let gate = teardown.clone();
    let factory: Factory = Arc::new(move || {
        let first = p.starts.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(Box::new(ControlledWorker {
            inner: FixtureWorker(p.clone()),
            admission: None,
            teardown: first.then(|| gate.clone()),
            fail_ensure: if first { 2 } else { 0 },
            ensures: 0,
        }))
    });
    let server = Server::with_factory(None, false, Some(factory));
    let _release = ReleaseOnDrop(teardown.clone());
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| teardown.arrived.load(Ordering::SeqCst));
    let progress = server.request("GET", "/status", "").1;
    assert_eq!(progress["state"], "stopped");
    assert!(progress.get("currentTraineeName").is_none());
    assert_eq!(server.request("GET", "/report", "").1, Value::Null);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 503);
    assert_eq!(server.request("GET", "/form-options", "").0, 503);
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(server.request("POST", path, "{}").0, 409);
    }
    assert!(probe.submissions().is_empty());
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    teardown.release();
    wait_until(|| !server.daemon.state.lock().unwrap().busy);
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    server.wait_for(|p| p["state"] == "complete");
    assert_eq!(probe.submissions(), ["fixture-1", "fixture-2"]);
}

#[test]
fn successful_worker_teardown_also_keeps_admission_closed() {
    let probe = Arc::new(Probe::default());
    let teardown = Gate::new();
    let p = probe.clone();
    let gate = teardown.clone();
    let factory: Factory = Arc::new(move || {
        p.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ControlledWorker {
            inner: FixtureWorker(p.clone()),
            admission: None,
            teardown: Some(gate.clone()),
            fail_ensure: 0,
            ensures: 0,
        }))
    });
    let server = Server::with_factory(None, false, Some(factory));
    let _release = ReleaseOnDrop(teardown.clone());
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| teardown.arrived.load(Ordering::SeqCst));
    assert_eq!(server.request("GET", "/status", "").1["state"], "complete");
    for path in ["/batch/pause", "/batch/resume"] {
        assert_eq!(server.request("POST", path, "{}").0, 409);
    }
    assert_eq!(server.request("POST", "/batch", BATCH).0, 503);
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    teardown.release();
    wait_until(|| !server.daemon.state.lock().unwrap().busy);
}

#[test]
fn factory_and_admission_failures_release_busy_for_the_next_request() {
    for fail_ensure in [0, 1] {
        let probe = Arc::new(Probe::default());
        let p = probe.clone();
        let factory: Factory = Arc::new(move || {
            let first = p.starts.fetch_add(1, Ordering::SeqCst) == 0;
            if first && fail_ensure == 0 {
                return Err("synthetic spawn failure".into());
            }
            Ok(Box::new(ControlledWorker {
                inner: FixtureWorker(p.clone()),
                admission: None,
                teardown: None,
                fail_ensure: if first { fail_ensure } else { 0 },
                ensures: 0,
            }))
        });
        let server = Server::with_factory(None, false, Some(factory));
        assert_eq!(server.request("POST", "/batch", BATCH).0, 502);
        assert!(!server.daemon.state.lock().unwrap().busy);
        assert_eq!(server.request("GET", "/status", "").1["state"], "stopped");
        assert!(probe.submissions().is_empty());
        assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
        server.wait_for(|p| p["state"] == "complete");
    }
}

#[test]
fn production_connection_cap_closes_excess_and_recovers_after_clients_leave() {
    let server = Server::new(None, false);
    let mut clients = Vec::new();
    for _ in 0..CONNECTION_LIMIT {
        clients.push(TcpStream::connect(server.address).unwrap());
    }
    wait_until(|| server.active.load(Ordering::SeqCst) == CONNECTION_LIMIT);
    let mut excess = TcpStream::connect(server.address).unwrap();
    excess
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut byte = [0u8];
    match excess.read(&mut byte) {
        Ok(n) => assert_eq!(n, 0),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset),
    }
    assert_eq!(server.active.load(Ordering::SeqCst), CONNECTION_LIMIT);
    for client in clients {
        client.shutdown(Shutdown::Both).unwrap();
    }
    wait_until(|| server.active.load(Ordering::SeqCst) == 0);
    assert_eq!(server.request("GET", "/status", "").0, 200);
    assert_eq!(server.probe.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn handler_panic_releases_connection_permit_and_busy() {
    let probe = Arc::new(Probe::default());
    let p = probe.clone();
    let factory: Factory = Arc::new(move || {
        assert_ne!(
            p.starts.fetch_add(1, Ordering::SeqCst),
            0,
            "synthetic handler panic"
        );
        Ok(Box::new(FixtureWorker(p.clone())))
    });
    let server = Server::with_factory(None, false, Some(factory));
    let mut stream = TcpStream::connect(server.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .write_all(batch_wire(server.address, BATCH).as_bytes())
        .unwrap();
    let mut discarded = Vec::new();
    let _ = stream.read_to_end(&mut discarded);
    wait_until(|| server.active.load(Ordering::SeqCst) == 0);
    assert!(!server.daemon.state.lock().unwrap().busy);
    assert_eq!(server.request("GET", "/status", "").1["state"], "stopped");
    assert_eq!(server.request("GET", "/form-options", "").0, 200);
}

struct PanicOnSubmit(FixtureWorker);
impl Transport for PanicOnSubmit {
    fn send(&mut self, request: &Request) -> Result<Response, String> {
        assert_ne!(
            request.op,
            crate::protocol::op::SUBMIT_TRAINING,
            "synthetic batch panic"
        );
        self.0.send(request)
    }
}

#[test]
fn batch_panic_clears_current_name_and_retains_checkpoint_refusal() {
    let probe = Arc::new(Probe::default());
    let p = probe.clone();
    let factory: Factory = Arc::new(move || {
        p.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(PanicOnSubmit(FixtureWorker(p.clone()))))
    });
    let server = Server::with_factory(None, false, Some(factory));
    assert_eq!(server.request("POST", "/batch", BATCH).0, 202);
    wait_until(|| !server.daemon.state.lock().unwrap().busy);
    let status = server.request("GET", "/status", "").1;
    assert_eq!(status["state"], "stopped");
    assert!(status.get("currentTraineeName").is_none());
    assert_eq!(
        server.request("GET", "/checkpoint", "").1["inFlight"],
        "fixture-1"
    );
    assert_eq!(server.request("POST", "/batch", BATCH).0, 409);
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
}
