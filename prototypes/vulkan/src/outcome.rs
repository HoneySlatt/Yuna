//! The answer of one check: passed, failed at a step, or skipped with its reason.

use crate::gpu::Failure;
use crate::json::Json;

pub enum Check {
    Passed(String),
    Failed(Failure),
    Skipped(String),
}

impl Check {
    pub fn from_result(result: Result<String, Failure>) -> Self {
        match result {
            Ok(detail) => Self::Passed(detail),
            Err(failure) => Self::Failed(failure),
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            Self::Passed(detail) => Json::Object(vec![
                ("result", Json::text("passed")),
                ("detail", Json::text(detail)),
            ]),
            Self::Failed(failure) => Json::Object(vec![
                ("result", Json::text("failed")),
                ("step", Json::text(&failure.step)),
                ("error", Json::text(&failure.error)),
            ]),
            Self::Skipped(reason) => Json::Object(vec![
                ("result", Json::text("skipped")),
                ("reason", Json::text(reason)),
            ]),
        }
    }

    pub fn to_cell(&self) -> String {
        let text = match self {
            Self::Passed(detail) if detail.is_empty() => "passed".to_owned(),
            Self::Passed(detail) => format!("passed: {detail}"),
            Self::Failed(failure) => format!("failed: {}", failure.describe()),
            Self::Skipped(reason) => format!("skipped: {reason}"),
        };
        text.replace('|', "\\|")
    }
}
