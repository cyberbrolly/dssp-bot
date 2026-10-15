//! Loopback HTTP bridge. Rust remains the sole owner of work and recovery.
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::checkpoint::{BatchCheckpoint, CheckpointWriter};
use crate::engine::{BatchControl, BatchEngine, Transport};
use crate::protocol::{Request, Response, SessionInput, Status, TraineeRef};
use crate::report::{BatchReport, Outcome};
use crate::store::CheckpointStore;
use serde::Deserialize;
use serde_json::{Value, json};

const BODY_LIMIT: usize = 64 * 1024;
const HEADER_LIMIT: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECTION_LIMIT: usize = 16;
type Factory = Arc<dyn Fn() -> Result<Box<dyn Transport + Send>, String> + Send + Sync>;

#[derive(Clone)]
struct Config {
    port: u16,
    origins: Vec<String>,
    token: String,
    checkpoint: PathBuf,
}

fn token_file(path: &Path) -> Result<String, String> {
    // Linux O_NOFOLLOW | O_NONBLOCK: refuse symlinks and do not hang
    // opening a substituted FIFO before the regular-file check.
    let open = || {
        OpenOptions::new()
            .read(true)
            .custom_flags(0x20000 | 0x800)
            .open(path)
    };
    let mut file = match open() {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut random = [0u8; 32];
            fs::File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut random))
                .map_err(|e| format!("token entropy: {e}"))?;
            let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
            {
                Ok(mut f) => {
                    f.write_all(token.as_bytes())
                        .and_then(|_| f.sync_all())
                        .map_err(|e| format!("write token: {e}"))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(format!("create token {}: {e}", path.display())),
            }
            open().map_err(|e| format!("read token: {e}"))?
        }
        Err(e) => return Err(format!("read token {}: {e}", path.display())),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let own_uid = fs::metadata("/proc/self").map_err(|e| e.to_string())?.uid();
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.uid() != own_uid
        || metadata.nlink() != 1
    {
        return Err("token must be an owned, unlinked regular file with mode 0600".into());
    }
    let mut token = String::new();
    Read::by_ref(&mut file)
        .take(65)
        .read_to_string(&mut token)
        .map_err(|e| e.to_string())?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("token must contain exactly 64 hexadecimal characters".into());
    }
    Ok(token)
}

impl Config {
    fn environment() -> Result<Self, String> {
        let port = std::env::var("DSSP_DAEMON_PORT")
            .unwrap_or_else(|_| "8765".into())
            .parse::<u16>()
            .map_err(|_| "DSSP_DAEMON_PORT must be a port number")?;
        if port == 0 {
            return Err("DSSP_DAEMON_PORT must not be zero".into());
        }
        let origins: Vec<String> = std::env::var("DSSP_DAEMON_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if origins.is_empty() || origins.iter().any(|s| !valid_origin(s)) {
            return Err("DSSP_DAEMON_ORIGINS requires comma-separated exact chrome-extension://id or moz-extension://uuid Origins (no paths/wildcards/whitespace)".into());
        }
        // WorkerClient launches from python/, so relative profile overrides
        // must be interpreted there too, regardless of the daemon's cwd.
        let python_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("python");
        let configured_profile = std::env::var_os("DSSP_PROFILE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".pw-profile"));
        let profile = if configured_profile.is_absolute() {
            configured_profile
        } else {
            python_dir.join(configured_profile)
        };
        let parent = profile
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let token_path = parent.join("dssp.daemon.token");
        let token = token_file(&token_path)?;
        eprintln!(
            "dssp-bot: daemon token file → {} (paste manually; never exposed by HTTP)",
            token_path.display()
        );
        if std::env::var_os(crate::recovery::RESUME_ENV).is_some() {
            eprintln!(
                "dssp-bot: DSSP_RESUME is ignored for HTTP requests; use the request resume boolean"
            );
        }
        Ok(Self {
            port,
            origins,
            token,
            checkpoint: CheckpointStore::for_job(Path::new("job.json"))
                .path()
                .into(),
        })
    }
}

fn valid_origin(origin: &str) -> bool {
    ["chrome-extension://", "moz-extension://"]
        .iter()
        .any(|prefix| {
            origin.strip_prefix(prefix).is_some_and(|id| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        })
}

#[derive(Default)]
struct State {
    busy: bool,
    progress: Value,
    report: Option<BatchReport>,
    control: Option<Arc<BatchControl>>,
}

fn progress(
    state: &str,
    total: usize,
    results: &[crate::report::TrainingResult],
    name: Option<&str>,
) -> Value {
    let count = |outcome| results.iter().filter(|r| r.outcome == outcome).count();
    let mut value = json!({"state": state, "total": total, "processed": results.len(),
        "successful": count(Outcome::Success), "failed": count(Outcome::Failed),
        "skipped": count(Outcome::Skipped), "indeterminate": count(Outcome::Indeterminate),
        "remaining": total.saturating_sub(results.len()), "pauseRequested": false,
                "stopRequested": false});
    if let Some(name) = name {
        value["currentTraineeName"] = json!(name);
    }
    value
}

// HTTP-only casing adapter; durable Rust schemas and CLI output are untouched.
fn camel(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let mut words = key.split('_');
                    let mut key = words.next().unwrap_or_default().to_owned();
                    for word in words {
                        let mut chars = word.chars();
                        if let Some(c) = chars.next() {
                            key.extend(c.to_uppercase());
                            key.extend(chars);
                        }
                    }
                    (key, camel(value))
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(camel).collect()),
        value => value,
    }
}

struct Sink {
    store: CheckpointStore,
    state: Arc<Mutex<State>>,
    names: HashMap<String, String>,
}
impl CheckpointWriter for Sink {
    fn write(&mut self, cp: &BatchCheckpoint) -> Result<(), String> {
        self.store.save(cp).map_err(|e| {
            eprintln!("dssp-bot: daemon checkpoint could not be written: {e}");
            e
        })?;
        let phase = match cp.status {
            crate::checkpoint::CheckpointStatus::Finished => "complete",
            crate::checkpoint::CheckpointStatus::Aborted => "stopped",
            crate::checkpoint::CheckpointStatus::Paused => "paused",
            _ if cp.in_flight.is_some() => "submitting",
            _ => "loading_trainee",
        };
        let name = cp.in_flight.as_ref().and_then(|id| self.names.get(id));
        let mut state = self.state.lock().unwrap();
        let requested = state.progress["pauseRequested"].clone();
        let stop_requested = state.progress["stopRequested"].clone();
        state.progress = progress(phase, cp.total, &cp.results, name.map(String::as_str));
        state.progress["stopRequested"] = stop_requested;
        if matches!(phase, "complete" | "stopped") {
            state.control = None;
        } else {
            state.progress["pauseRequested"] = requested;
        }
        Ok(())
    }
}

struct Bridge(Box<dyn Transport + Send>);
impl Transport for Bridge {
    fn send(&mut self, request: &Request) -> Result<Response, String> {
        self.0.send(request)
    }
}

fn clear_control(state: &mut State) {
    state.control = None;
    state.progress["pauseRequested"] = json!(false);
    if matches!(
        state.progress["state"].as_str(),
        Some("initializing" | "submitting" | "loading_trainee" | "retrying" | "paused")
    ) {
        state.progress["state"] = json!("stopped");
        state
            .progress
            .as_object_mut()
            .unwrap()
            .remove("currentTraineeName");
    }
}

struct Busy(Arc<Mutex<State>>);
impl Drop for Busy {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap();
        clear_control(&mut state);
        state.busy = false;
    }
}

// Unwinding closes controls before the worker's possibly blocking teardown,
// while WorkerLease retains Busy until that teardown has actually ended.
struct ControlLease(Arc<Mutex<State>>);
impl Drop for ControlLease {
    fn drop(&mut self) {
        clear_control(&mut self.0.lock().unwrap());
    }
}

// Field drop order keeps admission closed through worker shutdown, including
// unwinding and a failed thread spawn. The browser profile is still owned then.
struct WorkerLease {
    _control: ControlLease,
    worker: Bridge,
    _busy: Busy,
}

struct ConnectionPermit(Arc<AtomicUsize>);
impl ConnectionPermit {
    fn acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                (count < CONNECTION_LIMIT).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(active.clone()))
    }
}
impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Daemon {
    config: Config,
    state: Arc<Mutex<State>>,
    factory: Factory,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchRequest {
    trainees: Vec<TraineeRef>,
    session: SessionInput,
    #[serde(default)]
    resume: bool,
}

type Reply = (u16, Value);
fn error(code: u16, message: impl Into<String>) -> Reply {
    (code, json!({"error": message.into()}))
}
impl Daemon {
    fn new(config: Config, factory: Factory) -> Self {
        Self {
            config,
            factory,
            state: Arc::new(Mutex::new(State {
                progress: progress("idle", 0, &[], None),
                ..State::default()
            })),
        }
    }
    fn acquire(&self) -> Result<Busy, Reply> {
        let mut state = self.state.lock().unwrap();
        if state.busy {
            return Err(error(
                503,
                "a batch or form-options worker is already running",
            ));
        }
        state.busy = true;
        Ok(Busy(self.state.clone()))
    }
    fn route(&self, request: HttpRequest) -> Reply {
        if !equal_token(
            request
                .headers
                .get("authorization")
                .map(String::as_str)
                .unwrap_or(""),
            &format!("Bearer {}", self.config.token),
        ) {
            return error(401, "valid bearer token required");
        }
        if request
            .headers
            .get("origin")
            .is_some_and(|o| !self.config.origins.contains(o))
        {
            return error(403, "Origin is not allowed");
        }
        if request.method == "POST"
            && matches!(
                request.path.as_str(),
                "/batch" | "/batch/pause" | "/batch/resume" | "/batch/stop"
            )
            && !request.headers.get("content-type").is_some_and(|s| {
                s.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("application/json")
            })
        {
            return error(415, "Content-Type application/json required");
        }
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/status") => (200, self.state.lock().unwrap().progress.clone()),
            ("GET", "/report") => (
                200,
                camel(serde_json::to_value(&self.state.lock().unwrap().report).unwrap()),
            ),
            ("GET", "/checkpoint") => match CheckpointStore::new(&self.config.checkpoint).load() {
                Ok(cp) => (200, camel(serde_json::to_value(cp).unwrap())),
                Err(e) => error(500, e),
            },
            ("GET", "/form-options") => self.form_options(),
            ("POST", "/batch") => {
                let req: BatchRequest = match serde_json::from_slice(&request.body) {
                    Ok(req) => req,
                    Err(e) => return error(422, format!("invalid batch: {e}")),
                };
                self.start(req)
            }
            ("POST", "/batch/pause" | "/batch/resume" | "/batch/stop") => {
                match serde_json::from_slice::<Value>(&request.body) {
                    Ok(Value::Object(map)) if map.is_empty() => {}
                    _ => return error(422, "control body must be JSON {}"),
                }
                self.apply_control(if request.path == "/batch/stop" {
                    None
                } else {
                    Some(request.path == "/batch/pause")
                })
            }
            _ => error(405, "method/endpoint not implemented"),
        }
    }
    #[cfg(test)]
    fn control(&self, pause: bool) -> Reply {
        self.apply_control(Some(pause))
    }
    fn apply_control(&self, pause: Option<bool>) -> Reply {
        let control = self.state.lock().unwrap().control.clone();
        let Some(control) = control else {
            return error(
                409,
                "no controllable batch (idle, finished, form-options, or admission)",
            );
        };
        self.control_captured(&control, pause)
    }
    fn control_captured(&self, control: &Arc<BatchControl>, pause: Option<bool>) -> Reply {
        let mut requested = control
            .pause_requested
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut state = self.state.lock().unwrap();
        // A command that captured an old controller must not affect a new batch,
        // nor a worker whose result is terminal but whose teardown still owns Busy.
        if !state
            .control
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, control))
            || control.closed.load(Ordering::SeqCst)
        {
            return error(409, "batch is no longer controllable");
        }
        if let Some(pause) = pause {
            if control.stop_requested.load(Ordering::SeqCst) {
                return error(409, "operator stop is already requested");
            }
            *requested = pause;
            state.progress["pauseRequested"] = json!(pause);
        } else {
            control.stop_requested.store(true, Ordering::SeqCst);
            *requested = false;
            state.progress["pauseRequested"] = json!(false);
            state.progress["stopRequested"] = json!(true);
        }
        control.changed.notify_all();
        (200, state.progress.clone())
    }

    fn form_options(&self) -> Reply {
        let _busy = match self.acquire() {
            Ok(b) => b,
            Err(e) => return e,
        };
        let result = (|| {
            let mut worker = Bridge((self.factory)()?);
            ensure(&mut worker)?;
            let resp = worker.send(&Request::get_form_options(crate::new_id(), None))?;
            if resp.status != Status::Ok {
                return Err(resp.summary());
            }
            let options = |values: Option<Vec<crate::protocol::FormOption>>| {
                values
                    .unwrap_or_default()
                    .into_iter()
                    .map(|o| json!({"value": o.value, "label": o.label}))
                    .collect::<Vec<_>>()
            };
            Ok(
                json!({"instructors": options(resp.instructors), "trainingTypes": options(resp.training_types)}),
            )
        })();
        match result {
            Ok(value) => (200, value),
            Err(e) => error(502, e),
        }
    }
    fn start(&self, req: BatchRequest) -> Reply {
        let busy = match self.acquire() {
            Ok(b) => b,
            Err(e) => return e,
        };
        let store = CheckpointStore::new(&self.config.checkpoint);
        // Must precede every factory call, even when resume has no work left.
        let start = match crate::recovery::guard(&store, req.resume) {
            Ok(s) => s,
            Err(e) => return error(409, e),
        };
        let resumed = matches!(&start.plan, crate::recovery::Plan::Resume { .. });
        let (wanted, carried, owed) = match start.plan {
            crate::recovery::Plan::Fresh => (req.trainees, Vec::new(), 0),
            crate::recovery::Plan::Resume {
                roster,
                carried,
                owed,
            } => (roster, carried, owed),
        };
        if wanted.is_empty() && !resumed {
            return error(422, "trainees is empty");
        }
        if wanted
            .iter()
            .any(|t| crate::key_of(t).is_none_or(|key| key.trim().is_empty()))
            || [
                &req.session.training_date,
                &req.session.instructor,
                &req.session.training_type,
            ]
            .iter()
            .any(|s| s.trim().is_empty())
        {
            return error(
                422,
                "trainees need an id/name and session fields must not be empty",
            );
        }
        let batch_id = crate::new_id();
        if wanted.is_empty() {
            let previous = start.previous.as_ref().expect("resume has a predecessor");
            let report = BatchReport::build(
                carried,
                previous.started_at.clone(),
                previous.updated_at.clone(),
                owed > 0,
            );
            let mut state = self.state.lock().unwrap();
            state.progress = progress(
                if owed > 0 { "stopped" } else { "complete" },
                report.total,
                &report.results,
                None,
            );
            state.report = Some(report);
            return (202, json!({"batchId": batch_id}));
        }
        {
            let mut state = self.state.lock().unwrap();
            state.progress = progress("initializing", wanted.len() + carried.len(), &carried, None);
            state.report = None;
        }
        let mut worker = match (self.factory)() {
            Ok(w) => Bridge(w),
            Err(e) => return error(502, e),
        };
        if let Err(e) = ensure(&mut worker) {
            return error(502, e);
        }
        let list = match worker.send(&Request::list_trainees(crate::new_id())) {
            Ok(r) if r.status == Status::Ok => r.trainees.unwrap_or_default(),
            Ok(r) => return error(502, r.summary()),
            Err(e) => return error(502, e),
        };
        let resolved = match crate::resolve_against(&list, &wanted) {
            Ok(r) => r,
            Err(problems) => {
                return error(
                    422,
                    format!(
                        "cannot resolve {} of {} trainee(s):\n  - {}",
                        problems.len(),
                        wanted.len(),
                        problems.join("\n  - ")
                    ),
                );
            }
        };
        let names = list.into_iter().map(|t| (t.id, t.name)).collect();
        let state = self.state.clone();
        let control = Arc::new(BatchControl::default());
        state.lock().unwrap().control = Some(control.clone());
        let lease = WorkerLease {
            _control: ControlLease(state.clone()),
            worker,
            _busy: busy,
        };
        let spawn = std::thread::Builder::new()
            .name("dssp-batch".into())
            .spawn(move || {
                let mut lease = lease;

                let sink = Sink {
                    store,
                    state: state.clone(),
                    names,
                };
                let mut engine = BatchEngine::new(Box::new(sink))
                    .with_carried(carried)
                    .with_control(control);
                match engine.run(&mut lease.worker, &req.session, resolved) {
                    Ok(mut report) => {
                        // Missing records can be owed even when every runnable
                        // trainee succeeded; do not present that as a clean run.
                        report.aborted |= owed > 0;
                        let mut state = state.lock().unwrap();
                        state.control = None;
                        let stop_requested = state.progress["stopRequested"].clone();
                        state.progress = progress(
                            if report.aborted || owed > 0 {
                                "stopped"
                            } else {
                                "complete"
                            },
                            report.total,
                            &report.results,
                            None,
                        );
                        state.progress["stopRequested"] = stop_requested;
                        state.report = Some(report);
                    }
                    Err(e) => {
                        eprintln!("dssp-bot: daemon batch aborted: {e}");
                        let mut state = state.lock().unwrap();
                        state.control = None;
                        state.progress["pauseRequested"] = json!(false);
                        state.progress["state"] = json!("stopped");
                        state
                            .progress
                            .as_object_mut()
                            .unwrap()
                            .remove("currentTraineeName");
                    }
                }
            });
        match spawn {
            Ok(_) => (202, json!({"batchId": batch_id})),
            Err(e) => error(500, format!("batch thread: {e}")),
        }
    }
    fn dispatch(
        self: &Arc<Self>,
        stream: TcpStream,
        active: &Arc<AtomicUsize>,
    ) -> std::io::Result<Option<std::thread::JoinHandle<()>>> {
        let Some(permit) = ConnectionPermit::acquire(active) else {
            return Ok(None); // Close excess connections without creating a thread.
        };
        let daemon = self.clone();
        std::thread::Builder::new()
            .name("dssp-http".into())
            .spawn(move || {
                let _permit = permit;
                daemon.connection(stream);
            })
            .map(Some)
    }

    fn connection(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
        let reply = match read_request(&mut stream) {
            Ok(req) => self.route(req),
            Err(reply) => reply,
        };
        let body = serde_json::to_vec(&reply.1).unwrap();
        let reason = match reply.0 {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            409 => "Conflict",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Content",
            503 => "Service Unavailable",
            _ => "Server Error",
        };
        let header = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
            reply.0,
            reason,
            body.len()
        );
        let _ = stream
            .write_all(header.as_bytes())
            .and_then(|_| stream.write_all(&body));
    }
}

fn ensure(worker: &mut impl Transport) -> Result<(), String> {
    let resp = worker.send(&Request::ensure_session(crate::new_id()))?;
    if resp.status == Status::Ok {
        Ok(())
    } else {
        Err(resp.summary())
    }
}
fn equal_token(a: &str, b: &str) -> bool {
    let mut diff = a.len() ^ b.len();
    for (index, expected) in b.bytes().enumerate() {
        diff |= usize::from(a.as_bytes().get(index).copied().unwrap_or(0) ^ expected);
    }
    diff == 0
}
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}
fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, Reply> {
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut data = Vec::new();
    // Read headers without overreading, reject duplicate headers and ambiguous framing.
    while !data.ends_with(b"\r\n\r\n") {
        if data.len() >= HEADER_LIMIT {
            return Err(error(413, "headers too large"));
        }
        read_piece(stream, &mut data, 1, deadline)?;
    }
    let text = std::str::from_utf8(&data).map_err(|_| error(400, "invalid headers"))?;
    let mut lines = text.split("\r\n");
    let first: Vec<_> = lines.next().unwrap_or_default().split(' ').collect();
    if first.len() != 3
        || first[2] != "HTTP/1.1"
        || first[0].is_empty()
        || !first[0].bytes().all(http_token)
        || !first[1].starts_with('/')
        || !first[1].bytes().all(|b| b.is_ascii_graphic() && b != b'#')
    {
        return Err(error(400, "invalid request line"));
    }
    let method = first[0].to_owned();
    let path = first[1].to_owned();
    let mut headers = HashMap::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| error(400, "invalid header"))?;
        if key.is_empty()
            || !key.bytes().all(http_token)
            || value.bytes().any(|b| (b < 32 && b != b'\t') || b >= 127)
        {
            return Err(error(400, "invalid header"));
        }
        if headers
            .insert(key.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err(error(400, "duplicate header"));
        }
    }
    if !headers
        .get("host")
        .is_some_and(|host| !host.is_empty() && host.bytes().all(|b| b.is_ascii_graphic()))
    {
        return Err(error(400, "one nonempty Host header required"));
    }
    if headers.contains_key("transfer-encoding") || headers.contains_key("expect") {
        return Err(error(400, "unsupported framing"));
    }
    let size = match headers.get("content-length") {
        Some(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => s
            .parse::<usize>()
            .map_err(|_| error(413, "body too large"))?,
        Some(_) => return Err(error(400, "invalid Content-Length")),
        None => 0,
    };
    if size > BODY_LIMIT {
        return Err(error(413, "body exceeds 64 KB"));
    }
    let mut body = Vec::with_capacity(size);
    while body.len() < size {
        let left = size - body.len();
        read_piece(stream, &mut body, left.min(4096), deadline)?;
    }
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}
fn http_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn read_piece(
    stream: &mut TcpStream,
    data: &mut Vec<u8>,
    size: usize,
    deadline: Instant,
) -> Result<(), Reply> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| error(408, "request read timeout"))?;
    stream
        .set_read_timeout(Some(remaining))
        .map_err(|e| error(400, e.to_string()))?;
    let mut buf = vec![0; size];
    match stream.read(&mut buf) {
        Ok(0) => Err(error(400, "incomplete request")),
        Ok(n) => {
            data.extend_from_slice(&buf[..n]);
            Ok(())
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            Err(error(408, "request read timeout"))
        }
        Err(e) => Err(error(400, e.to_string())),
    }
}

pub fn run() -> Result<(), String> {
    let config = Config::environment()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))
        .map_err(|e| format!("bind loopback: {e}"))?;
    eprintln!(
        "dssp-bot: daemon listening at http://{}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    let daemon = Arc::new(Daemon::new(
        config,
        Arc::new(|| Ok(Box::new(crate::connect()?))),
    ));
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let stream = stream.map_err(|e| e.to_string())?;
        if let Err(e) = daemon.dispatch(stream, &active) {
            eprintln!("dssp-bot: connection thread could not start: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
