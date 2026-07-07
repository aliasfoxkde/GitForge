//! Unified error handling for GitForce
//!
//! Uses thiserror for library errors and anyhow for contextual errors.
//! Each error has a kind for programmatic error handling.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Error kinds for programmatic error handling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Entity not found
    NotFound,
    /// Authentication required or failed
    Authentication,
    /// Authorization denied
    Authorization,
    /// Already exists
    AlreadyExists,
    /// Invalid input or configuration
    InvalidInput,
    /// Database error
    Database,
    /// Git protocol error
    GitProtocol,
    /// Git repository error
    GitRepo,
    /// Event system error
    EventSystem,
    /// Storage error
    Storage,
    /// Sandbox execution error
    Sandbox,
    /// Network error
    Network,
    /// Timeout error
    Timeout,
    /// Internal error (bugs)
    Internal,
    /// Cancellation requested
    Cancelled,
}

/// Unified error type for GitForce
#[derive(Debug, Error)]
#[error("{kind}: {message}")]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    /// Create a new error with kind and message
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    /// Create a new error with kind, message, and source
    pub fn with_source<E>(kind: ErrorKind, message: impl Into<String>, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    /// Create a not found error
    pub fn not_found(entity: &str, id: impl std::fmt::Display) -> Self {
        Self::new(ErrorKind::NotFound, format!("{} not found: {}", entity, id))
    }

    /// Create an already exists error
    pub fn already_exists(entity: &str, name: impl std::fmt::Display) -> Self {
        Self::new(
            ErrorKind::AlreadyExists,
            format!("{} already exists: {}", entity, name),
        )
    }

    /// Create an invalid input error
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, msg)
    }

    /// Create an internal error
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, msg)
    }

    /// Create a database error
    pub fn database(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Database, msg)
    }

    /// Create a git error
    pub fn git(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::GitRepo, msg)
    }

    /// Create an authentication error
    pub fn auth(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Authentication, msg)
    }

    /// Create an authorization error
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Authorization, msg)
    }

    /// Create a storage error
    pub fn storage(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Storage, msg)
    }

    /// Create a sandbox error
    pub fn sandbox(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Sandbox, msg)
    }

    /// Create a timeout error
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, msg)
    }

    /// Create a cancelled error
    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "operation cancelled")
    }

    /// Create an event system error
    pub fn event_system(msg: impl Into<String>) -> Self {
        Self::new(ErrorKind::EventSystem, msg)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::NotFound => write!(f, "not_found"),
            ErrorKind::Authentication => write!(f, "authentication"),
            ErrorKind::Authorization => write!(f, "authorization"),
            ErrorKind::AlreadyExists => write!(f, "already_exists"),
            ErrorKind::InvalidInput => write!(f, "invalid_input"),
            ErrorKind::Database => write!(f, "database"),
            ErrorKind::GitProtocol => write!(f, "git_protocol"),
            ErrorKind::GitRepo => write!(f, "git_repo"),
            ErrorKind::EventSystem => write!(f, "event_system"),
            ErrorKind::Storage => write!(f, "storage"),
            ErrorKind::Sandbox => write!(f, "sandbox"),
            ErrorKind::Network => write!(f, "network"),
            ErrorKind::Timeout => write!(f, "timeout"),
            ErrorKind::Internal => write!(f, "internal"),
            ErrorKind::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Result type alias using GitForce's Error
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let err = Error::not_found("repository", "abc-123");
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(err.message.contains("repository"));
        assert!(err.message.contains("abc-123"));
    }

    #[test]
    fn test_error_display() {
        let err = Error::invalid_input("name cannot be empty");
        let display = format!("{}", err);
        assert!(display.contains("invalid_input"));
        assert!(display.contains("name cannot be empty"));
    }

    #[test]
    fn test_error_source() {
        let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = Error::with_source(ErrorKind::Storage, "failed to read", inner);
        assert!(err.source.is_some());
    }
}
