//! Supervisor for the Python worker: spawn, handshake, request/response.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::protocol::{Request, Response, Status};

pub struct WorkerClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl WorkerClient {
    /// Spawn `python/.venv/bin/python -m app.worker` from the repo root.
    ///
    /// `DSSP_WORKER_PY` overrides the interpreter path (tests use this to
    /// point at a scratch venv).
    pub fn spawn() -> Result<Self, String> {
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

        let mut child = Command::new(&python)
            .arg("-m")
            .arg("app.worker")
            .current_dir(&python_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn worker {}: {e}", python.display()))?;

        // Worker logs arrive on stderr; relay them so they surface in ours.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[worker] {line}");
                }
            });
        }

        let stdin = child.stdin.take().ok_or("worker stdin unavailable")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("worker stdout unavailable")?);

        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout,
        })
    }

    /// Wait for the startup handshake line `{"v":1,"status":"ready"}`.
    pub fn wait_ready(&mut self) -> Result<(), String> {
        let resp = self.read_response()?;
        match resp.status {
            Status::Ready => Ok(()),
            other => Err(format!("expected ready handshake, got {other:?}")),
        }
    }

    /// Send one request and read its response. Enforces the `job_id` echo.
    pub fn send(&mut self, req: &Request) -> Result<Response, String> {
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

        let resp = self.read_response()?;

        if resp.job_id != req.job_id {
            return Err(format!(
                "job_id mismatch: sent {}, received {} — protocol violation",
                req.job_id, resp.job_id
            ));
        }

        Ok(resp)
    }

    fn read_response(&mut self) -> Result<Response, String> {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| format!("read from worker failed: {e}"))?;
        if bytes == 0 {
            return Err("worker closed stdout".to_string());
        }
        let resp: Response = serde_json::from_str(line.trim())
            .map_err(|e| format!("invalid protocol line ({e}): {:?}", line.trim()))?;

        check_version(&resp)?;

        Ok(resp)
    }

    /// Close stdin (worker exits on EOF) and reap the child.
    pub fn shutdown(&mut self) {
        self.stdin.take(); // dropping the handle sends EOF
        let _ = self.child.wait();
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
        assert!(error.contains("speaks v2"), "names the worker's version: {error}");
        assert!(error.contains("speaks v1"), "and this build's: {error}");
    }

    /// The check is a comparison, not a guess, because `v` is the one field with no
    /// `#[serde(default)]`: a response that omits it fails to parse at all rather
    /// than defaulting to something this build would accept.
    #[test]
    fn a_response_that_omits_the_version_does_not_parse() {
        assert!(serde_json::from_str::<Response>(r#"{"status":"ok"}"#).is_err());
    }
}
