//! Authentication and authorization for the Git Smart HTTP transport.
//!
//! Every git route (`info/refs`, `git-upload-pack`, `git-receive-pack`,
//! legacy and standard spellings) funnels through [`authorize_git_http`]
//! before any repository data is touched. The contract:
//!
//! - **Credential**: `Authorization: Basic <username:credential>` where the
//!   credential is a live GitForge refresh credential (`gfrt…`, minted at
//!   `auth login`, 30-day TTL, revocable). Identity is resolved from the
//!   credential's stored digest — the username field is advisory, exactly
//!   like a GitHub PAT. Validation is read-only: git use does not rotate the
//!   credential (rotation stays with the API login/refresh path), so
//!   concurrent git operations and credential helpers never invalidate each
//!   other. JWTs are not accepted: the git-server holds no `JWT_SECRET`, and
//!   keeping it that way means no secret distribution to a second service.
//! - **Reads** (upload-pack): anonymous only when the repository is
//!   explicitly public; authenticated callers additionally follow the REST
//!   API's `can_access_repo` rule (owner, admin, or maintainer) for private
//!   repositories.
//! - **Writes** (receive-pack): always authenticated and always
//!   owner/admin/maintainer — anonymous writes fail closed.
//! - **No existence leak**: anonymous callers get a uniform `401` Basic
//!   challenge whether the repository is private or nonexistent (the
//!   challenge is also what makes `git` consult its credential helper);
//!   authenticated callers get a uniform `404` for nonexistent and
//!   unauthorized-private repositories, mirroring the API's
//!   `resolve_policy_repo` collapse.
//! - **Fail closed**: without a database nothing can be validated or
//!   authorized, and the transport answers `503` rather than serving.

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use gitforge_common::UserId;
use gitforge_db::models::Repository;
use gitforge_db::queries::{RefreshTokenQueries, RepoQueries, UserQueries};
use gitforge_db::Pool;
use std::sync::Arc;

/// The transport operation a request is attempting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitOperation {
    /// Fetch/clone: ref advertisement and upload-pack data.
    Read,
    /// Push: receive-pack advertisement and pack data.
    Write,
}

/// The authenticated account a request is attributed to.
#[derive(Debug, Clone)]
pub struct Actor {
    pub user_id: UserId,
    pub username: String,
    pub role: String,
}

impl Actor {
    /// The API's repository-access rule (`can_access_repo` in
    /// gitforge-api): the owner plus the two administrative roles may
    /// operate on any repository.
    fn may_manage_repos(&self, owner_id: UserId) -> bool {
        self.user_id == owner_id || matches!(self.role.as_str(), "admin" | "maintainer")
    }
}

/// Verdict for one git HTTP request.
#[derive(Debug)]
pub enum GitHttpAccess {
    /// Serve the request against this repository, optionally attributing
    /// it to an authenticated actor.
    Authorized {
        repo: Repository,
        actor: Option<Actor>,
    },
    /// `401` with a Basic challenge — anonymous or unacceptable
    /// credentials, uniform across private and nonexistent repositories.
    Challenge,
    /// `404` — authenticated caller, repository nonexistent or private
    /// without access. Collapsed so existence is not leaked.
    NotFound,
    /// `403` — authenticated caller, public repository, insufficient role
    /// for the requested operation (only writes can land here; a public
    /// repository's existence is already public).
    Forbidden,
    /// `503` — the database is unavailable; nothing can be authorized.
    Unavailable,
}

impl GitHttpAccess {
    /// Render the denial as a response, or `None` when the request is
    /// authorized and the caller should proceed. Mirrors `finish_response`
    /// in main.rs: builder inputs are static or pre-validated, and a
    /// construction failure degrades to a bare 500 instead of unwinding
    /// the connection handler.
    pub fn denial_response(&self, repo_path: &str) -> Option<Response> {
        let (status, body) = match self {
            GitHttpAccess::Authorized { .. } => return None,
            // The challenge is what makes a standard git client retry with
            // credentials from the URL or its credential helper.
            GitHttpAccess::Challenge => (
                StatusCode::UNAUTHORIZED,
                "authentication required\n".to_string(),
            ),
            GitHttpAccess::NotFound => (
                StatusCode::NOT_FOUND,
                format!("Repository not found: {repo_path}"),
            ),
            GitHttpAccess::Forbidden => (
                StatusCode::FORBIDDEN,
                format!("write access denied: {repo_path}"),
            ),
            GitHttpAccess::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Database not available".to_string(),
            ),
        };
        let mut builder = Response::builder().status(status);
        if matches!(self, GitHttpAccess::Challenge) {
            builder = builder.header(header::WWW_AUTHENTICATE, "Basic realm=\"GitForge\"");
        }
        let response = builder.body(Body::from(body)).unwrap_or_else(|error| {
            tracing::error!(%error, "git auth denial response construction failed");
            Response::new(Body::from("internal error"))
        });
        Some(response)
    }
}

/// What the `Authorization` header resolved to.
enum CredentialState {
    Anonymous,
    Authenticated(Actor),
    /// Presented but not acceptable (malformed, unknown, expired, revoked,
    /// or the account no longer exists). Never silently downgraded to
    /// anonymous — a client that tried to authenticate must see the
    /// challenge so a broken credential helper is visible.
    Invalid,
    /// The database failed while validating.
    Unavailable,
}

/// Validate the presented Basic credential against the refresh-credential
/// registry. The credential plaintext is never logged and never enters an
/// error message; only its digest crosses into the database layer.
async fn authenticate(pool: &Pool, headers: &HeaderMap) -> CredentialState {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return CredentialState::Anonymous;
    };
    let Some(encoded) = value.strip_prefix("Basic ") else {
        return CredentialState::Invalid;
    };
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let Some(decoded) = decoded else {
        return CredentialState::Invalid;
    };
    // Identity comes from the credential row, not the username field; the
    // username is split off only to satisfy the Basic shape. An empty
    // password half is never a valid presentation.
    let Some((_username, credential)) = decoded.split_once(':') else {
        return CredentialState::Invalid;
    };
    if credential.is_empty() {
        return CredentialState::Invalid;
    }

    let digest = RefreshTokenQueries::credential_digest(credential);
    let row = match RefreshTokenQueries::find_active_by_hash(pool, &digest).await {
        Ok(Some(row)) => row,
        Ok(None) => return CredentialState::Invalid,
        Err(error) => {
            tracing::error!(%error, "credential registry lookup failed");
            return CredentialState::Unavailable;
        }
    };
    let user = match UserQueries::get(pool, row.user_id).await {
        Ok(Some(user)) => user,
        Ok(None) => return CredentialState::Invalid,
        Err(error) => {
            tracing::error!(%error, user_id = %row.user_id, "credential user lookup failed");
            return CredentialState::Unavailable;
        }
    };
    // Resolve the persisted role so demotions take effect immediately,
    // mirroring the API auth middleware.
    let role = match UserQueries::get_role(pool, row.user_id).await {
        Ok(Some(role)) => role,
        Ok(None) => return CredentialState::Invalid,
        Err(error) => {
            tracing::error!(%error, user_id = %row.user_id, "credential role lookup failed");
            return CredentialState::Unavailable;
        }
    };
    CredentialState::Authenticated(Actor {
        user_id: row.user_id,
        username: user.username,
        role,
    })
}

/// Authenticate the request and authorize it against the repository named
/// by `owner`/`repo_name` for `operation`. See the module docs for the
/// contract; every git HTTP handler calls this before touching storage.
pub async fn authorize_git_http(
    db_pool: &Option<Arc<Pool>>,
    headers: &HeaderMap,
    owner: &str,
    repo_name: &str,
    operation: GitOperation,
) -> GitHttpAccess {
    let Some(pool) = db_pool else {
        return GitHttpAccess::Unavailable;
    };

    let state = authenticate(pool, headers).await;
    // Bad credentials are challenged before any repository lookup: the
    // response must not depend on repository existence for callers whose
    // credentials did not verify.
    let actor = match state {
        CredentialState::Anonymous => None,
        CredentialState::Authenticated(actor) => Some(actor),
        CredentialState::Invalid => {
            tracing::warn!(owner, repo = repo_name, "git http credential rejected");
            return GitHttpAccess::Challenge;
        }
        CredentialState::Unavailable => return GitHttpAccess::Unavailable,
    };

    let repo = match RepoQueries::get_by_owner_and_name(pool, owner, repo_name).await {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return match actor {
                // Anonymous callers get the uniform challenge so a private
                // repository and a nonexistent one are indistinguishable.
                None => GitHttpAccess::Challenge,
                Some(_) => GitHttpAccess::NotFound,
            };
        }
        Err(error) => {
            tracing::error!(%error, owner, repo = repo_name, "git http repository lookup failed");
            return GitHttpAccess::Unavailable;
        }
    };

    match (actor, operation) {
        (None, GitOperation::Read) if repo.is_public() => {
            GitHttpAccess::Authorized { repo, actor: None }
        }
        (None, _) => GitHttpAccess::Challenge,
        (Some(actor), _) if actor.may_manage_repos(repo.owner_id) => GitHttpAccess::Authorized {
            repo,
            actor: Some(actor),
        },
        (Some(actor), GitOperation::Read) if repo.is_public() => GitHttpAccess::Authorized {
            repo,
            actor: Some(actor),
        },
        // A public repository's existence is already public; a 403 states
        // the boundary instead of pretending the repository is missing.
        (Some(actor), GitOperation::Write) if repo.is_public() => {
            tracing::warn!(
                owner,
                repo = repo_name,
                user_id = %actor.user_id,
                role = %actor.role,
                "git http write denied"
            );
            GitHttpAccess::Forbidden
        }
        // Private repository, authenticated caller without access: 404, the
        // same answer a nonexistent repository produces.
        (Some(actor), _) => {
            tracing::warn!(
                owner,
                repo = repo_name,
                user_id = %actor.user_id,
                role = %actor.role,
                "git http access denied for private repository"
            );
            GitHttpAccess::NotFound
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use gitforge_db::models::User;

    fn header_map(value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(value).expect("test header value"),
            );
        }
        headers
    }

    fn basic(username: &str, credential: &str) -> String {
        use base64::Engine as _;
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{credential}"))
        )
    }

    /// Digest that intentionally re-derives the SHA-256 hex the issuance
    /// path computes, independent of `credential_digest`, so a drift in the
    /// shared helper breaks a test instead of silently aligning with
    /// itself.
    fn independent_digest(value: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        let digest = hasher.finalize();
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Migrated in-memory pool with the credential user `gituser` (role
    /// configurable), a second user `otherowner`, and a live refresh
    /// credential for `gituser`. Returns (pool, credential plaintext).
    async fn fixture(role: &str) -> (Pool, String) {
        let pool = Pool::memory().await.expect("memory pool");
        pool.migrate().await.expect("migrations");
        let user = User::new(
            "gituser".to_string(),
            "gituser@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user)
            .await
            .expect("create user");
        sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role)
            .bind(user.id.to_string())
            .execute(pool.pool())
            .await
            .expect("set role");
        let other = User::new(
            "otherowner".to_string(),
            "other@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &other)
            .await
            .expect("create other user");
        let credential = format!(
            "gfrt{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        RefreshTokenQueries::create(
            &pool,
            user.id,
            &independent_digest(&credential),
            chrono::Utc::now() + chrono::Duration::days(30),
        )
        .await
        .expect("seed credential");
        (pool, credential)
    }

    /// How the request presents itself.
    enum Present {
        Anonymous,
        ValidCredential,
        Raw(&'static str),
    }

    /// One row of the access truth table: seed `owner_name/target` with
    /// `visibility`, give the credential user `role`, and authorize.
    async fn authorize_case(
        owner_name: &str,
        visibility: &str,
        role: &str,
        operation: GitOperation,
        present: Present,
    ) -> GitHttpAccess {
        let (pool, credential) = fixture(role).await;
        let owner = UserQueries::get_by_username(&pool, owner_name)
            .await
            .expect("owner lookup")
            .expect("owner exists");
        let repository = Repository::new("target".to_string(), owner.id, "/git/target".to_string());
        RepoQueries::create(&pool, &repository)
            .await
            .expect("create repo");
        sqlx::query("UPDATE repositories SET visibility = ? WHERE id = ?")
            .bind(visibility)
            .bind(repository.id.to_string())
            .execute(pool.pool())
            .await
            .expect("set visibility");

        let header = match present {
            Present::Anonymous => None,
            Present::ValidCredential => Some(basic("gituser", &credential)),
            Present::Raw(value) => Some(value.to_string()),
        };
        let headers = header_map(header.as_deref());
        let pool = Arc::new(pool);
        authorize_git_http(&Some(pool), &headers, owner_name, "target", operation).await
    }

    #[test]
    fn test_credential_digest_matches_issuance_format() {
        // Cross-checked against an independent SHA-256 implementation so
        // the shared digest cannot drift from the format the API issuance
        // path defined.
        assert_eq!(
            RefreshTokenQueries::credential_digest("gfrt-test-vector"),
            independent_digest("gfrt-test-vector")
        );
        assert_eq!(RefreshTokenQueries::credential_digest("").len(), 64);
    }

    // --- credential presentations ---------------------------------------

    #[tokio::test]
    async fn test_malformed_credentials_are_challenged_not_anonymous() {
        // Even against a public read (which anonymous would satisfy), a
        // presented-but-broken credential must not fall back to anonymous.
        let access = authorize_case(
            "gituser",
            "public",
            "developer",
            GitOperation::Read,
            Present::Raw("Basic !!!not-base64!!!"),
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_non_basic_scheme_is_challenged() {
        let access = authorize_case(
            "gituser",
            "public",
            "developer",
            GitOperation::Read,
            Present::Raw("Bearer some.jwt.value"),
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_missing_colon_is_challenged() {
        // base64("gitusernocolon") — decodes fine but has no ':' separator.
        let access = authorize_case(
            "gituser",
            "public",
            "developer",
            GitOperation::Read,
            Present::Raw("Basic Z2l0dXNlcm5vY29sb24="),
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_unknown_credential_is_challenged() {
        // base64("gituser:gfrtnot-a-real-credential")
        let access = authorize_case(
            "gituser",
            "public",
            "developer",
            GitOperation::Write,
            Present::Raw("Basic Z2l0dXNlcjpnZnJ0bm90LWEtcmVhbC1jcmVkZW50aWFs"),
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_expired_credential_is_challenged() {
        let pool = Pool::memory().await.expect("memory pool");
        pool.migrate().await.expect("migrations");
        let user = User::new(
            "expired".to_string(),
            "expired@example.com".to_string(),
            "hash".to_string(),
        );
        UserQueries::create(&pool, &user).await.expect("user");
        let credential = "gfrt-expired-credential";
        RefreshTokenQueries::create(
            &pool,
            user.id,
            &RefreshTokenQueries::credential_digest(credential),
            chrono::Utc::now() - chrono::Duration::hours(1),
        )
        .await
        .expect("seed expired credential");

        let pool = Arc::new(pool);
        let headers = header_map(Some(&basic("expired", credential)));
        let access = authorize_git_http(
            &Some(pool),
            &headers,
            "expired",
            "whatever",
            GitOperation::Read,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_revoked_credential_is_challenged() {
        let (pool, credential) = fixture("developer").await;
        RefreshTokenQueries::revoke(&pool, &RefreshTokenQueries::credential_digest(&credential))
            .await
            .expect("revoke");
        let pool = Arc::new(pool);
        let headers = header_map(Some(&basic("gituser", &credential)));
        let access = authorize_git_http(
            &Some(pool),
            &headers,
            "gituser",
            "whatever",
            GitOperation::Read,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_missing_database_is_unavailable() {
        let headers = header_map(None);
        let access = authorize_git_http(&None, &headers, "a", "b", GitOperation::Read).await;
        assert!(matches!(access, GitHttpAccess::Unavailable));
        let response = access.denial_response("a/b").expect("denial renders");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_challenge_response_carries_basic_challenge_header() {
        let response = GitHttpAccess::Challenge
            .denial_response("owner/repo")
            .expect("denial renders");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok()),
            Some("Basic realm=\"GitForge\"")
        );
    }

    // --- the access truth table ------------------------------------------
    //
    // Anonymous+public+read is the ONLY anonymous allow; writes always
    // need an account that may manage the repository.

    #[tokio::test]
    async fn test_anonymous_public_read_allowed() {
        let access = authorize_case(
            "gituser",
            "public",
            "developer",
            GitOperation::Read,
            Present::Anonymous,
        )
        .await;
        assert!(matches!(
            access,
            GitHttpAccess::Authorized { actor: None, .. }
        ));
    }

    #[tokio::test]
    async fn test_anonymous_public_write_challenged() {
        let access = authorize_case(
            "gituser",
            "public",
            "developer",
            GitOperation::Write,
            Present::Anonymous,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_anonymous_private_read_challenged() {
        let access = authorize_case(
            "gituser",
            "private",
            "developer",
            GitOperation::Read,
            Present::Anonymous,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Challenge));
    }

    #[tokio::test]
    async fn test_owner_private_read_and_write_allowed() {
        for operation in [GitOperation::Read, GitOperation::Write] {
            let access = authorize_case(
                "gituser",
                "private",
                "developer",
                operation,
                Present::ValidCredential,
            )
            .await;
            assert!(
                matches!(access, GitHttpAccess::Authorized { actor: Some(_), .. }),
                "owner must be authorized for {operation:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_admin_and_maintainer_write_others_private_repo() {
        for role in ["admin", "maintainer"] {
            let access = authorize_case(
                "otherowner",
                "private",
                role,
                GitOperation::Write,
                Present::ValidCredential,
            )
            .await;
            assert!(
                matches!(access, GitHttpAccess::Authorized { .. }),
                "{role} must manage another owner's private repo"
            );
        }
    }

    #[tokio::test]
    async fn test_developer_cannot_read_others_private_repo() {
        let access = authorize_case(
            "otherowner",
            "private",
            "developer",
            GitOperation::Read,
            Present::ValidCredential,
        )
        .await;
        // 404, not 403: a private repository the caller cannot read must be
        // indistinguishable from a nonexistent one.
        assert!(matches!(access, GitHttpAccess::NotFound));
        let response = access
            .denial_response("otherowner/target")
            .expect("renders");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_developer_cannot_write_others_public_repo() {
        let access = authorize_case(
            "otherowner",
            "public",
            "developer",
            GitOperation::Write,
            Present::ValidCredential,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Forbidden));
        let response = access
            .denial_response("otherowner/target")
            .expect("renders");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_read_only_role_cannot_write_others_public_repo() {
        let access = authorize_case(
            "otherowner",
            "public",
            "read_only",
            GitOperation::Write,
            Present::ValidCredential,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Forbidden));
    }

    #[tokio::test]
    async fn test_developer_can_read_others_public_repo() {
        let access = authorize_case(
            "otherowner",
            "public",
            "developer",
            GitOperation::Read,
            Present::ValidCredential,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::Authorized { .. }));
    }

    #[tokio::test]
    async fn test_authenticated_missing_repo_is_not_found() {
        let (pool, credential) = fixture("admin").await;
        let pool = Arc::new(pool);
        let headers = header_map(Some(&basic("gituser", &credential)));
        let access = authorize_git_http(
            &Some(pool),
            &headers,
            "gituser",
            "no-such-repo",
            GitOperation::Read,
        )
        .await;
        assert!(matches!(access, GitHttpAccess::NotFound));
    }
}
