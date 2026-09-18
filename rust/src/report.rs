//! Batch result + report types. Ports TrainingResult.ts and BatchReport.ts.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    Failed,
    /// Never attempted — drained after a batch abort.
    Skipped,
    /// Submitted but unconfirmed. Needs manual verification.
    Indeterminate,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainingResult {
    pub trainee_id: String,
    pub trainee_name: String,
    pub outcome: Outcome,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchReport {
    pub total: usize,
    pub successful: usize,
    pub failed: usize,
    pub skipped: usize,
    pub indeterminate: usize,
    pub success_rate: f64,
    pub started_at: String,
    pub finished_at: String,
    pub results: Vec<TrainingResult>,
}

impl BatchReport {
    pub fn build(results: Vec<TrainingResult>, started_at: String, finished_at: String) -> Self {
        let successful = results
            .iter()
            .filter(|r| r.outcome == Outcome::Success)
            .count();
        let failed = results
            .iter()
            .filter(|r| r.outcome == Outcome::Failed)
            .count();
        let skipped = results
            .iter()
            .filter(|r| r.outcome == Outcome::Skipped)
            .count();
        let indeterminate = results
            .iter()
            .filter(|r| r.outcome == Outcome::Indeterminate)
            .count();
        let total = results.len();
        let success_rate = if total == 0 {
            0.0
        } else {
            successful as f64 / total as f64
        };

        Self {
            total,
            successful,
            failed,
            skipped,
            indeterminate,
            success_rate,
            started_at,
            finished_at,
            results,
        }
    }

    pub fn summary_line(&self) -> String {
        format!(
            "total={} success={} failed={} skipped={} indeterminate={} rate={:.0}%",
            self.total,
            self.successful,
            self.failed,
            self.skipped,
            self.indeterminate,
            self.success_rate * 100.0
        )
    }
}
