//! Result type alias for GitForce
//!
//! Provides a convenient Result type alias using GitForce's Error type.

pub use super::error::Error;

/// Result type alias using GitForce's Error
pub type Result<T> = std::result::Result<T, Error>;

/// Extension trait for Result operations
pub trait ResultExt<T, E> {
    /// Map the error to a different type using a closure
    fn map_err_context<C, F>(self, f: F) -> std::result::Result<T, C>
    where
        F: FnOnce(E) -> C;

    /// Convert to a different error type
    fn context<C>(self, context: C) -> std::result::Result<T, C>
    where
        E: Into<C>;
}

impl<T, E> ResultExt<T, E> for std::result::Result<T, E> {
    fn map_err_context<C, F>(self, f: F) -> std::result::Result<T, C>
    where
        F: FnOnce(E) -> C,
    {
        self.map_err(f)
    }

    fn context<C>(self, context: C) -> std::result::Result<T, C>
    where
        E: Into<C>,
    {
        self.map_err(Into::into)
    }
}
