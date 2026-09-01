//! Local administrative bootstrap operations.

use anyhow::{bail, Context, Result};
use gitforge_common::password::hash_password;
use gitforge_db::{models::User, queries::UserQueries, Pool};

const MIN_PASSWORD_LENGTH: usize = 12;

/// Create the first administrator in a freshly initialized GitForge database.
///
/// This deliberately refuses to run once any administrator exists. It is a
/// local bootstrap operation, not a remote privilege-escalation mechanism.
pub async fn bootstrap_first_admin(
    database_url: &str,
    username: &str,
    email: &str,
    password: &str,
    confirmed: bool,
) -> Result<User> {
    if !confirmed {
        bail!("refusing bootstrap without --confirm");
    }
    let username = username.trim();
    let email = email.trim();
    if username.is_empty() || email.is_empty() {
        bail!("username and email must not be empty");
    }
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        bail!("password must be at least {MIN_PASSWORD_LENGTH} characters");
    }

    let pool = Pool::new(database_url)
        .await
        .context("failed to open GitForge database")?;
    pool.migrate()
        .await
        .context("failed to migrate GitForge database")?;
    if UserQueries::count_role(&pool, "admin").await? > 0 {
        bail!("an administrator already exists; bootstrap is disabled");
    }

    let user = User::new(
        username.to_string(),
        email.to_string(),
        hash_password(password).context("failed to hash administrator password")?,
    );
    UserQueries::create_with_role(&pool, &user, "admin").await?;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::bootstrap_first_admin;
    use gitforge_common::password::verify_password;
    use gitforge_db::{queries::UserQueries, Pool};
    use std::{env, path::PathBuf};

    fn test_artifact_directory() -> PathBuf {
        let directory = env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                env::current_dir()
                    .expect("test working directory should be available")
                    .join("target")
            });
        directory.join("gitforge-cli-admin-tests")
    }

    #[tokio::test]
    async fn creates_first_admin_and_never_replaces_it() {
        let artifact_directory = test_artifact_directory();
        std::fs::create_dir_all(&artifact_directory).unwrap();
        let directory = tempfile::tempdir_in(artifact_directory).unwrap();
        let database_url = format!("sqlite:{}/gitforge.db?mode=rwc", directory.path().display());
        let user = bootstrap_first_admin(
            &database_url,
            "operator",
            "operator@example.test",
            "a-strong-bootstrap-password",
            true,
        )
        .await
        .unwrap();

        let pool = Pool::new(&database_url).await.unwrap();
        pool.migrate().await.unwrap();
        let stored = UserQueries::get_by_username(&pool, "operator")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(UserQueries::count_role(&pool, "admin").await.unwrap(), 1);
        assert!(verify_password("a-strong-bootstrap-password", &stored.password_hash).unwrap());
        assert_eq!(stored.id, user.id);

        let second = bootstrap_first_admin(
            &database_url,
            "replacement",
            "replacement@example.test",
            "another-strong-password",
            true,
        )
        .await;
        assert!(second.is_err());
        assert_eq!(UserQueries::count_role(&pool, "admin").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn requires_confirmation_and_strong_password() {
        assert!(bootstrap_first_admin(
            "sqlite::memory:",
            "operator",
            "operator@example.test",
            "short",
            false,
        )
        .await
        .is_err());
        assert!(bootstrap_first_admin(
            "sqlite::memory:",
            "operator",
            "operator@example.test",
            "short",
            true,
        )
        .await
        .is_err());
    }
}
