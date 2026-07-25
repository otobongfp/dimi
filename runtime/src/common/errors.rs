use thiserror::Error;

pub type Result<T> = std::result::Result<T, DimiError>;

#[derive(Debug, Error)]
pub enum DimiError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("capability degraded: {0}")]
    Degraded(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

pub fn not_implemented<T>(what: &str) -> Result<T> {
    Err(DimiError::NotImplemented(what.to_string()))
}
