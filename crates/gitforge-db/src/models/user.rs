//! User model

use chrono::{DateTime, Utc};
use gitforge_common::{SshKeyId, UserId};
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

/// A registered SSH public key belonging to one user. The OpenSSH
/// fingerprint (`SHA256:<base64>`) is the lookup identity used by the git
/// transport's public-key authentication; the raw `authorized_keys` line is
/// kept for display and for re-verifying the fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKey {
    pub id: SshKeyId,
    pub user_id: UserId,
    /// Human label, e.g. `laptop`.
    pub name: String,
    /// OpenSSH fingerprint of the public key (`SHA256:...`).
    pub fingerprint: String,
    /// The full OpenSSH public key line.
    pub public_key: String,
    pub created_at: DateTime<Utc>,
}

impl SshKey {
    /// Create a new SSH key record for `user_id`.
    pub fn new(user_id: UserId, name: String, fingerprint: String, public_key: String) -> Self {
        Self {
            id: SshKeyId::new(),
            user_id,
            name,
            fingerprint,
            public_key,
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

    #[test]
    fn test_ssh_key_creation() {
        let user_id = UserId::new();
        let key = SshKey::new(
            user_id,
            "laptop".to_string(),
            "SHA256:abc".to_string(),
            "ssh-ed25519 AAAA test".to_string(),
        );
        assert_eq!(key.user_id, user_id);
        assert_eq!(key.name, "laptop");
        assert_eq!(key.fingerprint, "SHA256:abc");
        assert!(key.public_key.starts_with("ssh-ed25519 "));
    }
}
