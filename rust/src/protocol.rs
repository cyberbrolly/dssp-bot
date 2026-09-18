//! Wire protocol shared with the Python worker (docs/protocol.md, v1).
//!
//! JSON, one object per line, over the worker's stdin/stdout. The `job_id` is
//! echoed on every response; stdout carries protocol lines only.

use serde::{Deserialize, Serialize};

pub const V: u32 = 1;

/// Operation names, mirroring app/protocol.py.
pub mod op {
    pub const PING: &str = "ping";
    pub const ENSURE_SESSION: &str = "ensure_session";
    pub const LIST_TRAINEES: &str = "list_trainees";
    pub const GET_FORM_OPTIONS: &str = "get_form_options";
    pub const SUBMIT_TRAINING: &str = "submit_training";
}

/// Trainee selector: by id when given, else by name. An ambiguous name match
/// is rejected by the worker — we must then supply the id, never guess.
#[derive(Debug, Clone, Serialize)]
pub struct TraineeRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Training details for one submission. `instructor`/`training_type` may be a
/// portal value or its label; the worker resolves either.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInput {
    pub training_date: String,
    pub instructor: String,
    pub training_type: String,
}

#[derive(Debug, Serialize)]
pub struct Request {
    pub v: u32,
    pub job_id: String,
    pub op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trainee_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trainee: Option<TraineeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInput>,
}

impl Request {
    fn new(job_id: impl Into<String>, op: &'static str) -> Self {
        Self {
            v: V,
            job_id: job_id.into(),
            op,
            trainee_id: None,
            trainee: None,
            session: None,
        }
    }

    pub fn ping(job_id: impl Into<String>) -> Self {
        Self::new(job_id, op::PING)
    }

    pub fn ensure_session(job_id: impl Into<String>) -> Self {
        Self::new(job_id, op::ENSURE_SESSION)
    }

    pub fn list_trainees(job_id: impl Into<String>) -> Self {
        Self::new(job_id, op::LIST_TRAINEES)
    }

    pub fn get_form_options(job_id: impl Into<String>, trainee_id: Option<String>) -> Self {
        let mut req = Self::new(job_id, op::GET_FORM_OPTIONS);
        req.trainee_id = trainee_id;
        req
    }

    pub fn submit_training(
        job_id: impl Into<String>,
        trainee: TraineeRef,
        session: SessionInput,
    ) -> Self {
        let mut req = Self::new(job_id, op::SUBMIT_TRAINING);
        req.trainee = Some(trainee);
        req.session = Some(session);
        req
    }
}

/// Status of a worker line: the startup handshake (`ready`) or a reply to a
/// request (`ok` / `error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Error,
    Ready,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FormOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TraineeInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sn: String,
    #[serde(default)]
    pub course: String,
    #[serde(default)]
    pub training_sessions: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TraineeResolved {
    pub id: String,
    pub name: String,
}

/// A parsed worker response. Field-level options keep deserialization robust
/// against ops we never asked about; `job_id` echoing is enforced by
/// [`crate::worker::WorkerClient::send`].
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    pub v: u32,
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub op: String,
    pub status: Status,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub proves_nothing_submitted: Option<bool>,
    #[serde(default)]
    pub pong: Option<bool>,
    #[serde(default)]
    pub authenticated: Option<bool>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub trainees: Option<Vec<TraineeInfo>>,
    #[serde(default)]
    pub instructors: Option<Vec<FormOption>>,
    #[serde(default)]
    pub training_types: Option<Vec<FormOption>>,
    #[serde(default)]
    pub trainee: Option<TraineeResolved>,
    #[serde(default)]
    pub attempts: Option<u32>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub reference: Option<String>,
}

impl Response {
    /// Human-readable summary for logs and the final report.
    pub fn summary(&self) -> String {
        match self.status {
            Status::Ok => {
                if let Some(outcome) = &self.outcome {
                    format!("ok outcome={outcome}")
                } else if let Some(count) = self.count {
                    format!("ok count={count}")
                } else {
                    "ok".to_string()
                }
            }
            Status::Error => format!(
                "error code={} proves_nothing_submitted={}",
                self.error_code.as_deref().unwrap_or("?"),
                self.proves_nothing_submitted.unwrap_or(false)
            ),
            Status::Ready => "ready".to_string(),
        }
    }
}
