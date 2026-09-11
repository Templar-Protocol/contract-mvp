use std::process::ExitCode;

use serde::Serialize;
use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("policy refused: {0}")]
    Policy(String),
    #[error("chain operation failed: {0}")]
    Chain(String),
    #[error("operation is ambiguous or conflicts with recorded state: {0}")]
    Conflict(String),
    #[error("custody or health invariant failed: {0}")]
    Custody(String),
    #[error("health checks failed")]
    Health(Vec<serde_json::Value>),
    #[error("I/O operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::Policy(message) if message == "unsupported_use_cctp" => "unsupported_use_cctp",
            Self::Policy(message) if message == "production_mutation_unsupported_v1" => {
                "production_mutation_unsupported_v1"
            }
            Self::Policy(_) => "policy_refused",
            Self::Chain(_) => "chain_failure",
            Self::Conflict(_) => "operation_conflict",
            Self::Custody(_) => "custody_failure",
            Self::Health(_) => "health_failure",
            Self::Io(_) => "io_failure",
            Self::Json(_) => "invalid_json",
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(match self {
            Self::InvalidInput(_) => 2,
            Self::Policy(_) => 3,
            Self::Chain(_) | Self::Io(_) | Self::Json(_) => 4,
            Self::Conflict(_) => 5,
            Self::Custody(_) | Self::Health(_) => 6,
        })
    }

    pub fn context(&self) -> serde_json::Value {
        match self {
            Self::Health(findings) => serde_json::json!({"findings": findings}),
            _ => serde_json::json!({}),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorBody<'a> {
    pub code: &'a str,
    pub message: String,
    pub context: serde_json::Value,
}
