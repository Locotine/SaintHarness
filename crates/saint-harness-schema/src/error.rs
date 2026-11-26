use crate::ToolResult;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum SchemaError {
    #[error("unknown model profile: {0}")]
    UnknownProfile(String),
    #[error("invalid TOML configuration: {0}")]
    InvalidToml(#[from] toml::de::Error),
    #[error("profile validation failed: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportOp {
    Connect,
    Send,
    Receive,
    Dns,
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AiError {
    #[error("context overflow: {message}")]
    ContextOverflow { message: String, raw_body: String },
    #[error("payload too large: {message}")]
    PayloadTooLarge { message: String },
    #[error("rate limited (retry_after_ms={retry_after_ms:?}): {message}")]
    RateLimit {
        message: String,
        retry_after_ms: Option<u64>,
        should_retry_header: Option<bool>,
    },
    #[error("quota exceeded: {message}")]
    QuotaExceeded { message: String },
    #[error("authentication failed: {message}")]
    Authentication { message: String },
    #[error("content policy violation: {message}")]
    ContentPolicy { message: String },
    #[error("provider internal error ({status:?}): {message}")]
    ProviderInternal {
        status: Option<u16>,
        message: String,
        retry_after_ms: Option<u64>,
        should_retry_header: Option<bool>,
    },
    #[error("transport error ({operation:?}, network_down={network_down}): {message}")]
    Transport {
        operation: TransportOp,
        accepted: bool,
        network_down: bool,
        message: String,
    },
    #[error("incomplete stream: {message}")]
    IncompleteStream { message: String },
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("timeout: {message}")]
    Timeout { message: String },
    #[error("unknown provider error: {message}")]
    UnknownProvider { message: String, raw_body: String },
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolFailure {
    #[error("blocked by policy: {reason}")]
    BlockedError { reason: String },
    #[error("rejected with user feedback: {feedback}")]
    CorrectedError { feedback: String },
    #[error("validation error: {message}")]
    ValidationError { message: String },
    #[error("runtime error: {message}")]
    RuntimeError { message: String },
    #[error("stale anchor error: {message}")]
    StaleAnchorError { message: String },
    #[error("interrupted: {message}")]
    Interrupted { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Completed(ToolResult),
    RecoverableError(ToolFailure),
    // Bare user rejection stops the active turn immediately and must never be coerced into RecoverableError.
    UserDeclined,
}
