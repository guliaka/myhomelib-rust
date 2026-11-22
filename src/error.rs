//! Application error type. Domain errors are surfaced to the UI as human-readable messages.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("http error: {0}")]
    Http(String),

    #[error("download cancelled")]
    Cancelled,

    #[error("bad credentials (no download redirect captured)")]
    WrongCredentials,

    #[error("download blocked by server")]
    BlockedByServer,

    #[error("no response to check")]
    NoResponse,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    /// A human-readable message error (the catch-all `Other` variant, for dynamic strings).
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Other(s.into())
    }
}
