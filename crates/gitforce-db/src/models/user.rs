//! User model

use chrono::{DateTime, Utc};
use gitforce_common::UserId;
use serde::{Deserialize, Serialize};

/// User entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

impl User {
    /// Create a new user
    pub fn new(username: String, email: String, password_hash: String) -> Self {
        Self {
            id: UserId::new(),
            username,
            email,
            password_hash,
            created_at: Utc::now(),
        }
    }
}

/// User role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Admin,
    Maintainer,
    Developer,
    ReadOnly,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Maintainer => "maintainer",
            Role::Developer => "developer",
            Role::ReadOnly => "read_only",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_creation() {
        let user = User::new(
            "testuser".to_string(),
            "test@example.com".to_string(),
            "hash123".to_string(),
        );
        assert_eq!(user.username, "testuser");
        assert_eq!(user.email, "test@example.com");
        assert_eq!(user.password_hash, "hash123");
    }

    #[test]
    fn test_role_as_str() {
        assert_eq!(Role::Admin.as_str(), "admin");
        assert_eq!(Role::Maintainer.as_str(), "maintainer");
        assert_eq!(Role::Developer.as_str(), "developer");
        assert_eq!(Role::ReadOnly.as_str(), "read_only");
    }
}
