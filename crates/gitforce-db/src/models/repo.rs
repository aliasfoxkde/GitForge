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
