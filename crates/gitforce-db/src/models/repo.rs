//! Repository model

use chrono::{DateTime, Utc};
use gitforce_common::{RepoId, UserId};
use serde::{Deserialize, Serialize};

/// Repository visibility
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private,
}

impl Visibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Private => "private",
        }
    }
}

/// Repository entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub id: RepoId,
    pub name: String,
    pub owner_id: UserId,
    pub visibility: String,
    pub git_path: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Repository {
    /// Create a new repository
    pub fn new(name: String, owner_id: UserId, git_path: String) -> Self {
        let now = Utc::now();
        Self {
            id: RepoId::new(),
            name,
            owner_id,
            visibility: Visibility::Private.as_str().to_string(),
            git_path,
            created_at: now,
            updated_at: now,
        }
    }

    /// Check if repository is public
    pub fn is_public(&self) -> bool {
        self.visibility == "public"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visibility_as_str() {
        assert_eq!(Visibility::Public.as_str(), "public");
        assert_eq!(Visibility::Private.as_str(), "private");
    }

    #[test]
    fn test_repository_creation() {
        let owner_id = UserId::new();
        let repo = Repository::new(
            "test-repo".to_string(),
            owner_id,
            "/git/test-repo".to_string(),
        );
        assert_eq!(repo.name, "test-repo");
        assert_eq!(repo.owner_id, owner_id);
        assert_eq!(repo.visibility, "private");
        assert!(!repo.is_public());
    }

    #[test]
    fn test_repository_public_visibility() {
        let owner_id = UserId::new();
        let mut repo = Repository::new(
            "public-repo".to_string(),
            owner_id,
            "/git/public-repo".to_string(),
        );
        repo.visibility = Visibility::Public.as_str().to_string();
        assert!(repo.is_public());
    }

    #[test]
    fn test_repository_id_unique() {
        let owner_id = UserId::new();
        let repo1 = Repository::new("repo1".to_string(), owner_id, "/git/repo1".to_string());
        let repo2 = Repository::new("repo2".to_string(), owner_id, "/git/repo2".to_string());
        assert_ne!(repo1.id, repo2.id);
    }
}
