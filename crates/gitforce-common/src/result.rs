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
    fn context<C>(self, _context: C) -> std::result::Result<T, C>
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

    fn context<C>(self, _context: C) -> std::result::Result<T, C>
    where
        E: Into<C>,
    {
        self.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_map_err_context() {
        let result: std::result::Result<i32, &str> = Err("original error");
        let mapped = result.map_err_context(|e| format!("wrapped: {}", e));
        assert!(mapped.is_err());
        assert_eq!(mapped.unwrap_err(), "wrapped: original error");
    }

    #[test]
    fn test_result_context() {
        // When E and C are the same type, context doesn't transform the error
        let result: std::result::Result<i32, &str> = Err("error");
        let with_context = result.context("context info");
        assert!(with_context.is_err());
        // E: Into<C> where both are &str just passes through
        assert_eq!(with_context.unwrap_err(), "error");
    }

    #[test]
    fn test_result_context_ok() {
        let result: std::result::Result<i32, &str> = Ok(42);
        let with_context = result.context("context info");
        assert!(with_context.is_ok());
        assert_eq!(with_context.unwrap(), 42);
    }

    #[test]
    fn test_result_map_err_context_ok() {
        let result: std::result::Result<i32, &str> = Ok(100);
        let mapped = result.map_err_context(|e| format!("error: {}", e));
        assert!(mapped.is_ok());
        assert_eq!(mapped.unwrap(), 100);
    }
}
