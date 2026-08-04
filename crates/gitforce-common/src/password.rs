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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hash_and_verify() {
        let password = "secure_password_123";
        let hash = hash_password(password).unwrap();

        // Hash should be different from password
        assert_ne!(hash, password);

        // Verify correct password
        assert!(verify_password(password, &hash).unwrap());

        // Verify wrong password
        assert!(!verify_password("wrong_password", &hash).unwrap());
    }

    #[test]
    fn test_password_hash_different_each_time() {
        let password = "same_password";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();

        // bcrypt should produce different hashes each time (due to salt)
        assert_ne!(hash1, hash2);

        // Both should verify correctly
        assert!(verify_password(password, &hash1).unwrap());
        assert!(verify_password(password, &hash2).unwrap());
    }

    #[test]
    fn test_empty_password() {
        let hash = hash_password("").unwrap();
        assert!(verify_password("", &hash).unwrap());
        assert!(!verify_password("not_empty", &hash).unwrap());
    }

    #[test]
    fn test_long_password() {
        let password = "a".repeat(100);
        let hash = hash_password(&password).unwrap();
        assert!(verify_password(&password, &hash).unwrap());
    }
}
