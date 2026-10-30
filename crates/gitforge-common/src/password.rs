//! Password hashing utilities
//!
//! Uses bcrypt for secure password hashing

/// Hash a password using bcrypt
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| PasswordError::HashError(e.to_string()))
}

/// Verify a password against a hash
pub fn verify_password(password: &str, hash: &str) -> Result<bool, PasswordError> {
    bcrypt::verify(password, hash).map_err(|e| PasswordError::VerifyError(e.to_string()))
}

/// Password error types
#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("failed to hash password: {0}")]
    HashError(String),

    #[error("failed to verify password: {0}")]
    VerifyError(String),
}
