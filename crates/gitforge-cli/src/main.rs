//! GitForge CLI
//!
//! Local-first Git platform client for GitForge.

use anyhow::{Context, Result};
use clap::Parser;
use clap::Subcommand;
use std::path::PathBuf;

mod admin;
mod client;
mod config;
mod review;
mod sync;

pub use client::GitForgeClient;
pub use config::Config;

#[derive(Parser, Debug)]
#[command(
    name = "gitforge",
    version = "0.3.0",
    about = "GitForge CLI - Local-first Git platform client",
    long_about = None,
    after_help = "For more information, see https://github.com/aliasfoxkde/GitForge"
)]
pub struct Cli {
    /// Enable verbose output
    #[arg(short, long)]
    verbose: bool,

    /// GitForge server URL
    #[arg(short, long)]
    server: Option<String>,

    /// Authentication token
    #[arg(short, long)]
    token: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, Clone)]
#[allow(non_camel_case_types)]
enum Commands {
    /// Authentication and user management
    Auth {
        /// Login with username
        #[arg(long)]
        login: Option<String>,
        /// Logout and clear credentials
        #[arg(long)]
        logout: bool,
        /// Show current authentication status
        #[arg(long)]
        status: bool,
        /// Show user info
        #[arg(long)]
        whoami: bool,
    },
    /// Local first-administrator bootstrap
    Admin {
        /// Create the first administrator in DATABASE_URL
        #[arg(long)]
        bootstrap: bool,
        /// Administrator username
        #[arg(long, requires = "bootstrap")]
        username: Option<String>,
        /// Administrator email
        #[arg(long, requires = "bootstrap")]
        email: Option<String>,
        /// Explicitly confirm local first-admin creation
        #[arg(long, requires = "bootstrap")]
        confirm: bool,
    },
    /// Repository operations
    Repo {
        /// List all repositories
        #[arg(short, long)]
        list: bool,
        /// Create a new repository
        #[arg(short, long)]
        create: Option<String>,
        /// Show repository information
        #[arg(short, long)]
        info: Option<String>,
        /// Delete a repository
        #[arg(short, long)]
        delete: Option<String>,
        /// Clone a repository to local directory
        #[arg(long)]
        clone: Option<String>,
        /// Initialize a local directory as a GitForge repo
        #[arg(long)]
        init: Option<String>,
    },
    /// Git operations (wrapper for standard git with GitForge remote)
    Git {
        /// Initialize a new git repository with GitForge remote
        #[arg(long)]
        init: Option<String>,
        /// Clone a repository
        #[arg(long)]
        clone: Option<String>,
        /// Show working tree status
        #[arg(long)]
        status: bool,
        /// Push commits to remote
        #[arg(long)]
        push: bool,
        /// Pull commits from remote
        #[arg(long)]
        pull: bool,
        /// Add files to staging
        #[arg(long)]
        add: Option<String>,
        /// Commit changes
        #[arg(long)]
        commit: Option<String>,
        /// Show commit history
        #[arg(long)]
        log: bool,
        /// Show remote information
        #[arg(long)]
        remote: bool,
    },
    /// Pipeline/CI-CD operations
    Pipeline {
        /// List all pipelines
        #[arg(short, long)]
        list: bool,
        /// Show pipeline details
        #[arg(short, long)]
        show: Option<String>,
        /// Trigger a pipeline run
        #[arg(short, long)]
        run: Option<String>,
        /// Watch pipeline execution
        #[arg(short, long)]
        watch: Option<String>,
        /// Create a new pipeline
        #[arg(short, long)]
        create: Option<String>,
        /// Delete a pipeline
        #[arg(short, long)]
        delete: Option<String>,
    },
    /// Runner agent management
    Runner {
        /// List registered runners
        #[arg(short, long)]
        list: bool,
        /// Show runner information
        #[arg(short, long)]
        info: Option<String>,
        /// Register a new runner
        #[arg(short, long)]
        register: Option<String>,
        /// Set runner capacity (number of concurrent jobs)
        #[arg(long)]
        capacity: Option<i32>,
        /// Deregister a runner
        #[arg(long)]
        deregister: Option<String>,
    },
    /// Cloud sync operations
    Sync {
        /// Show sync status
        #[arg(short, long)]
        status: bool,
        /// Push local state to remote
        #[arg(short, long)]
        push: bool,
        /// Pull remote state to local
        #[arg(short, long)]
        pull: bool,
        /// Initialize local storage
        #[arg(long)]
        init: Option<String>,
    },
    /// AI code review
    Review {
        /// Review staged changes (default)
        #[arg(long)]
        staged: bool,
        /// Use the provided diff text directly (requires --diff)
        #[arg(long)]
        diff: bool,
        /// Diff content (used with --diff flag)
        #[arg(long)]
        diff_content: Option<String>,
        /// Review changes against a specific base (e.g., main, HEAD~1)
        #[arg(long)]
        base: Option<String>,
        /// Git ref to review (branch, tag, SHA)
        #[arg(long)]
        target: Option<String>,
        /// Additional context (commit message, PR description)
        #[arg(long)]
        context: Option<String>,
        /// AI provider to use
        #[arg(long, default_value = "anthropic")]
        provider: String,
        /// Include verbose output
        #[arg(short, long)]
        verbose: bool,
    },
}

/// Run the CLI command handler (extracted for testing)
pub async fn run_cli(cli: Cli) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let server = cli.server.unwrap_or_else(|| config.server_url.clone());
    let token = cli.token.or_else(|| config.token.clone());

    match &cli.command {
        Commands::Auth {
            login,
            logout,
            status,
            whoami,
        } => {
            let api_client = GitForgeClient::new(&config.api_url(), token.clone());

            if let Some(username) = login {
                println!("🔐 Authenticating as {} to {}", username, server);
                println!("   Enter password: ");

                // Read password from stdin
                let password = rpassword::read_password().unwrap_or_default();

                match api_client.login(username, &password).await {
                    Ok(response) => {
                        println!("✅ Login successful!");
                        println!("   Token expires in {} seconds", response.expires_in);
                        println!();
                        println!(
                            "   Token: {}...",
                            &response.token[..response.token.len().min(20)]
                        );

                        // Save token to config
                        let mut config = config.clone();
                        config.token = Some(response.token);
                        if let Err(e) = config.save() {
                            tracing::warn!("Failed to save token: {}", e);
                        }
                    }
                    Err(e) => {
                        println!("❌ Login failed: {}", e);
                        println!("   Please check your username and password.");
                    }
                }
            } else if *logout {
                let mut config = config.clone();
                config.token = None;
                if let Err(e) = config.save() {
                    println!("⚠️  Warning: Failed to clear credentials: {}", e);
                }
                println!("👋 Logged out. Credentials cleared.");
                println!("   Run `gitforge auth login <username>` to authenticate again.");
            } else if *status {
                match api_client.auth_status().await {
                    Ok(status) => {
                        if status.authenticated {
                            println!("✅ Authenticated to {}", server);
                            println!("   Username: {}", status.username.unwrap_or_default());
                            if let Some(role) = status.role {
                                println!("   Role: {}", role);
                            }
                        } else {
                            println!("❌ Not authenticated.");
                            if let Some(msg) = status.message {
                                println!("   Reason: {}", msg);
                            }
                            println!("   Run `gitforge auth login <username>` to authenticate.");
                        }
                    }
                    Err(e) => {
                        println!("❌ Failed to check auth status: {}", e);
                        if token.is_some() {
                            println!("   You have a token but the server may be unreachable.");
                        }
                    }
                }
            } else if *whoami {
                match api_client.auth_status().await {
                    Ok(status) => {
                        if status.authenticated {
                            println!("👤 Authenticated user");
                            println!("   Server: {}", server);
                            println!("   User ID: {}", status.user_id.unwrap_or_default());
                            println!("   Username: {}", status.username.unwrap_or_default());
                            if let Some(role) = status.role {
                                println!("   Role: {}", role);
                            }
                        } else {
                            println!("❌ Not authenticated.");
                        }
                    }
                    Err(e) => {
                        println!("❌ Failed to get user info: {}", e);
                    }
                }
            }
        }

        Commands::Admin {
            bootstrap,
            username,
            email,
            confirm,
        } => {
            if !bootstrap {
                anyhow::bail!(
                    "select an administrative operation; currently supported: --bootstrap"
                );
            }
            let database_url = std::env::var("DATABASE_URL")
                .context("DATABASE_URL is required for local administrator bootstrap")?;
            let username = username
                .as_deref()
                .context("--username is required with --bootstrap")?;
            let email = email
                .as_deref()
                .context("--email is required with --bootstrap")?;
            let password = rpassword::prompt_password("Administrator password: ")?;
            let _user =
                admin::bootstrap_first_admin(&database_url, username, email, &password, *confirm)
                    .await?;
            println!("✅ First administrator created successfully.");
            println!("   Run `gitforge auth login --login <username>` to obtain a session token.");
        }

        Commands::Repo {
            list,
            create,
            info,
            delete,
            clone,
            init,
        } => {
            let api_client = GitForgeClient::new(&config.api_url(), token.clone());

            if *list {
                match api_client.list_repos().await {
                    Ok(repos) => {
                        println!("📦 Repositories on {}:", server);
                        println!();
                        if repos.is_empty() {
                            println!("  No repositories found.");
                        } else {
                            for repo in repos {
                                println!(
                                    "  {:20} - {} [{}]",
                                    repo.name, repo.git_path, repo.visibility
                                );
                            }
                        }
                        println!();
                        println!("  Run `gitforge repo create <name>` to create a new repository.");
                    }
                    Err(e) => {
                        println!("❌ Failed to list repositories: {}", e);
                    }
                }
            } else if let Some(name) = create {
                println!("📦 Creating repository '{}'...", name);
                match api_client
                    .create_repo(name, Some("private".to_string()))
                    .await
                {
                    Ok(repo) => {
                        println!("✅ Repository created successfully!");
                        println!("   ID: {}", repo.id);
                        println!("   Name: {}", repo.name);
                        println!("   Visibility: {}", repo.visibility);
                    }
                    Err(e) => {
                        println!("❌ Failed to create repository: {}", e);
                    }
                }
            } else if let Some(id) = info {
                match api_client.get_repo(id).await {
                    Ok(repo) => {
                        println!("📋 Repository: {}", repo.name);
                        println!("   ID: {}", repo.id);
                        println!("   Owner: {}", repo.owner_id);
                        println!("   Visibility: {}", repo.visibility);
                        println!("   Git Path: {}", repo.git_path);
                        println!("   Created: {}", repo.created_at);
                    }
                    Err(e) => {
                        println!("❌ Failed to get repository: {}", e);
                    }
                }
            } else if let Some(id) = delete {
                println!("⚠️  Deleting repository '{}'...", id);
                println!("   This action is irreversible!");
                match api_client.delete_repo(id).await {
                    Ok(_) => {
                        println!("✅ Repository deleted successfully.");
                    }
                    Err(e) => {
                        println!("❌ Failed to delete repository: {}", e);
                    }
                }
            } else if let Some(url_or_name) = clone {
                println!("📥 Cloning repository...");
                println!("   Source: {}", url_or_name);
                println!("   (Use `gitforge git clone <repo>` for actual cloning)");
            } else if let Some(path) = init {
                println!("🔧 Initializing directory as GitForge repository...");
                println!("   Path: {}", path);
                println!("   (API not yet wired)");
            }
        }

        Commands::Git {
            init,
            clone,
            status,
            push,
            pull,
            add,
            commit,
            log,
            remote,
        } => {
            if let Some(path) = init {
                let dir = PathBuf::from(path);
                println!("🔧 Initializing Git repository at {}...", dir.display());
                println!("   Adding GitForge remote...");
                println!("   (Actual git init would be performed here)");
            } else if let Some(url) = clone {
                println!("📥 Cloning from {}...", url);
                println!("   This will clone the repository to the current directory.");
                println!("   (Actual git clone would be performed here)");
                println!();
                println!("   GitForge supports:");
                println!("   - HTTPS cloning via GitForge API");
                println!("   - SSH cloning via git-server service");
            } else if *status {
                println!("📊 Git Status:");
                println!();
                println!("  On branch: main");
                println!("  Your branch is up to date with 'origin/main'.");
                println!();
                println!("  nothing to commit, working tree clean");
                println!();
                println!("  (This is a demo - actual git status would show real state)");
            } else if *push {
                println!("⬆️  Pushing to remote...");
                println!("   (Actual git push would be performed here)");
            } else if *pull {
                println!("⬇️  Pulling from remote...");
                println!("   (Actual git pull would be performed here)");
            } else if let Some(files) = add {
                if files == "." {
                    println!("📝 Staging all changes...");
                } else {
                    println!("📝 Staging: {}", files);
                }
                println!("   (Actual git add would be performed here)");
            } else if let Some(msg) = commit {
                println!("💾 Committing...");
                println!("   Message: {}", msg);
                println!("   (Actual git commit would be performed here)");
            } else if *log {
                println!("📜 Commit History:");
                println!();
                println!("  commit abc123 (HEAD -> main)");
                println!("  Author: User <user@example.com>");
                println!("  Date:   2026-07-22");
                println!();
                println!("      Initial commit");
                println!();
                println!("  (This is a demo - actual git log would show real history)");
            } else if *remote {
                println!("🔗 Git Remotes:");
                println!();
                println!("  origin  {} (fetch)", server);
                println!("  origin  {} (push)", server);
            }
        }

        Commands::Pipeline {
            list,
            show,
            run,
            watch,
            create,
            delete,
        } => {
            let api_client = GitForgeClient::new(&config.api_url(), token.clone());

            if *list {
                match api_client.list_pipelines().await {
                    Ok(pipelines) => {
                        println!("⚙️  Pipelines on {}:", server);
                        println!();
                        if pipelines.is_empty() {
                            println!("  No pipelines found.");
                        } else {
                            for pipeline in pipelines {
                                let status = if pipeline.enabled {
                                    "active"
                                } else {
                                    "disabled"
                                };
                                println!(
                                    "  {:20} - {} [{}]",
                                    pipeline.name, pipeline.repo_id, status
                                );
                            }
                        }
                        println!();
                        println!("  Run `gitforge pipeline create <name>` to create a pipeline.");
                    }
                    Err(e) => {
                        println!("❌ Failed to list pipelines: {}", e);
                    }
                }
            } else if let Some(id) = show {
                match api_client.get_pipeline(id).await {
                    Ok(pipeline) => {
                        println!("⚙️  Pipeline: {}", pipeline.name);
                        println!("   ID: {}", pipeline.id);
                        println!("   Repository: {}", pipeline.repo_id);
                        println!("   Enabled: {}", pipeline.enabled);
                    }
                    Err(e) => {
                        println!("❌ Failed to get pipeline: {}", e);
                    }
                }
            } else if let Some(id) = run {
                println!("🚀 Triggering pipeline: {}", id);
                println!("   (Pipeline trigger not yet implemented)");
            } else if let Some(id) = watch {
                println!("👁️  Watching pipeline: {}", id);
                println!("   (Pipeline watch not yet implemented)");
            } else if let Some(_name) = create {
                println!("⚙️  Creating pipeline...");
                println!("   (Pipeline creation not yet implemented)");
            } else if let Some(_id) = delete {
                println!("⚠️  Deleting pipeline...");
                println!("   (Pipeline deletion not yet implemented)");
            }
        }

        Commands::Runner {
            list,
            info,
            register,
            capacity,
            deregister,
        } => {
            let api_client = GitForgeClient::new(&config.api_url(), token.clone());

            if *list {
                match api_client.list_runners().await {
                    Ok(runners) => {
                        println!("🤖 Runners on {}:", server);
                        println!();
                        if runners.is_empty() {
                            println!("  No runners registered.");
                        } else {
                            for runner in runners {
                                println!(
                                    "  {:15} - {} [{}] capacity: {}",
                                    runner.name, runner.runner_type, runner.status, runner.capacity
                                );
                            }
                        }
                        println!();
                        println!("  Run `gitforge runner register <name>` to register a runner.");
                    }
                    Err(e) => {
                        println!("❌ Failed to list runners: {}", e);
                    }
                }
            } else if let Some(id) = info {
                match api_client.get_runner(id).await {
                    Ok(runner) => {
                        println!("🤖 Runner: {}", runner.name);
                        println!("   ID: {}", runner.id);
                        println!("   Type: {}", runner.runner_type);
                        println!("   Status: {}", runner.status);
                        println!("   Capacity: {}", runner.capacity);
                        if let Some(lhb) = runner.last_heartbeat {
                            println!("   Last Heartbeat: {}", lhb);
                        }
                    }
                    Err(e) => {
                        println!("❌ Failed to get runner: {}", e);
                    }
                }
            } else if let Some(name) = register {
                let cap = capacity.unwrap_or(2);
                println!("🤖 Registering runner: {}", name);
                println!("   Server: {}", server);
                println!("   Capacity: {} concurrent jobs", cap);
                println!("   (Runner registration not yet implemented)");
            } else if deregister.is_some() {
                let _id = deregister.clone().unwrap_or_default();
                println!("🤖 Deregistering runner...");
                println!("   (Runner deregistration not yet implemented)");
            } else if let Some(cap) = capacity {
                println!("🤖 Updating runner capacity to: {}", cap);
                println!("   (Runner capacity update not yet implemented)");
            }
        }

        Commands::Sync {
            status,
            push,
            pull,
            init,
        } => {
            let local_dir = config.local_data_dir.clone();

            if let Some(directory) = init {
                let path = PathBuf::from(directory);
                if path.exists() {
                    anyhow::bail!("Directory {} already exists", directory);
                }
                std::fs::create_dir_all(&path)?;
                let init_client = sync::SyncClient::with_real_client(path.clone());
                init_client.init().await?;
                println!("✅ Initialized local storage at {}", path.display());
                return Ok(());
            }

            if *status {
                println!("☁️  GitForge Sync Status");
                println!();
                println!("   Local storage: {}", local_dir.display());
                let sync_client = sync::SyncClient::with_real_client(local_dir.clone());
                let sync_status = sync_client.status().await;
                println!("   Sync state: {:?}", sync_status);
                println!();
                println!("   Server: {}", server);
                println!(
                    "   (Authenticated: {})",
                    if token.is_some() { "yes" } else { "no" }
                );
            } else if *push {
                if let Some(ref t) = token {
                    println!("⬆️  Pushing to remote...");
                    let sync_client = sync::SyncClient::with_real_client(local_dir.clone());
                    match sync_client.push(&config.api_url(), t).await {
                        Ok(response) => {
                            println!("✅ Push successful!");
                            println!("   Remote revision: {}", response.remote_rev);
                            if response.conflicts.is_empty() {
                                println!("   No conflicts");
                            } else {
                                println!("   Conflicts: {}", response.conflicts.join(", "));
                            }
                        }
                        Err(e) => {
                            tracing::error!("Push failed: {}", e);
                            println!("❌ Push failed: {}", e);
                        }
                    }
                } else {
                    println!("❌ Not authenticated. Run `gitforge auth login <username>` first.");
                }
            } else if *pull {
                if let Some(ref t) = token {
                    println!("⬇️  Pulling from remote...");
                    let sync_client = sync::SyncClient::with_real_client(local_dir.clone());
                    match sync_client.pull(&config.api_url(), t).await {
                        Ok(response) => {
                            println!("✅ Pull successful!");
                            println!("   Remote revision: {}", response.remote_rev);
                            println!("   Repositories synced: {}", response.repos.len());
                            println!("   Pipelines synced: {}", response.pipelines.len());
                        }
                        Err(e) => {
                            tracing::error!("Pull failed: {}", e);
                            println!("❌ Pull failed: {}", e);
                        }
                    }
                } else {
                    println!("❌ Not authenticated. Run `gitforge auth login <username>` first.");
                }
            }
        }

        Commands::Review {
            staged,
            diff,
            diff_content,
            base,
            target,
            context,
            provider,
            verbose,
        } => {
            use crate::review::{
                create_review_request, get_current_branch, get_git_diff, get_uncommitted_diff,
                print_complexity, print_diff_stats, print_review_results, provider_type_from_name,
                run_review,
            };

            let provider_type = provider_type_from_name(provider).map_err(|e| {
                eprintln!("❌ {}", e);
                anyhow::anyhow!("invalid provider")
            })?;

            let repo_path =
                std::env::current_dir().context("Could not determine current directory")?;

            let diff_text = if *diff {
                if let Some(ref content) = diff_content {
                    content.clone()
                } else {
                    println!("❌ --diff flag requires --diff-content to be provided.");
                    anyhow::bail!("--diff requires --diff-content");
                }
            } else if *staged || base.is_some() || target.is_some() {
                get_git_diff(&repo_path, base.as_deref(), target.as_deref())?
            } else {
                get_uncommitted_diff(&repo_path)?
            };

            if diff_text.trim().is_empty() {
                println!("✅ No changes to review.");
                return Ok(());
            }

            let branch = get_current_branch(&repo_path).unwrap_or_else(|_| "unknown".to_string());
            let repo_name = repo_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let context_str = context.as_deref().unwrap_or("");

            let request = create_review_request(
                &repo_name,
                &branch,
                base.as_deref(),
                &diff_text,
                context_str,
            )?;

            let changes = &request.files;
            print_diff_stats(&diff_text).ok();
            print_complexity(changes);
            println!();
            println!("🔍 Running {} AI review...", provider);

            match run_review(provider_type, &request).await {
                Ok(response) => {
                    print_review_results(&response, *verbose || cli.verbose);
                    if response.has_critical_findings() {
                        println!();
                        println!("⚠️  Review complete — critical findings detected.");
                    }
                }
                Err(e) => {
                    println!("❌ Review failed: {}", e);
                    anyhow::bail!("review generation failed: {}", e);
                }
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(if cli.verbose {
                tracing::Level::DEBUG.into()
            } else {
                tracing::Level::INFO.into()
            }),
        )
        .init();

    run_cli(cli).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cli(command: Commands) -> Cli {
        Cli {
            verbose: false,
            server: None,
            token: None,
            command,
        }
    }

    #[tokio::test]
    async fn test_auth_status_not_authenticated() {
        let cli = test_cli(Commands::Auth {
            login: None,
            logout: false,
            status: true,
            whoami: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_auth_login() {
        let cli = test_cli(Commands::Auth {
            login: Some("testuser".to_string()),
            logout: false,
            status: false,
            whoami: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_auth_logout() {
        let cli = test_cli(Commands::Auth {
            login: None,
            logout: true,
            status: false,
            whoami: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_repo_list() {
        let cli = test_cli(Commands::Repo {
            list: true,
            create: None,
            info: None,
            delete: None,
            clone: None,
            init: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_status() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: true,
            push: false,
            pull: false,
            add: None,
            commit: None,
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_log() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: false,
            push: false,
            pull: false,
            add: None,
            commit: None,
            log: true,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_remote() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: false,
            push: false,
            pull: false,
            add: None,
            commit: None,
            log: false,
            remote: true,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_list() {
        let cli = test_cli(Commands::Pipeline {
            list: true,
            show: None,
            run: None,
            watch: None,
            create: None,
            delete: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_runner_list() {
        let cli = test_cli(Commands::Runner {
            list: true,
            info: None,
            register: None,
            capacity: None,
            deregister: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_sync_status() {
        let cli = test_cli(Commands::Sync {
            status: true,
            push: false,
            pull: false,
            init: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_sync_init_creates_directory() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir
            .join("gitforge-test-init")
            .to_str()
            .unwrap()
            .to_string();
        let _ = std::fs::remove_dir_all(&test_dir);

        let cli = test_cli(Commands::Sync {
            status: false,
            push: false,
            pull: false,
            init: Some(test_dir.clone()),
        });
        assert!(run_cli(cli).await.is_ok());
        assert!(std::path::Path::new(&test_dir).exists());

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_git_init() {
        let cli = test_cli(Commands::Git {
            init: Some("/tmp/test-repo".to_string()),
            clone: None,
            status: false,
            push: false,
            pull: false,
            add: None,
            commit: None,
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_clone() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: Some("https://example.com/repo.git".to_string()),
            status: false,
            push: false,
            pull: false,
            add: None,
            commit: None,
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_push() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: false,
            push: true,
            pull: false,
            add: None,
            commit: None,
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_pull() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: false,
            push: false,
            pull: true,
            add: None,
            commit: None,
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_add() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: false,
            push: false,
            pull: false,
            add: Some(".".to_string()),
            commit: None,
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_add_specific_file() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: false,
            push: false,
            pull: false,
            add: Some("src/main.rs".to_string()),
            commit: None,
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_git_commit() {
        let cli = test_cli(Commands::Git {
            init: None,
            clone: None,
            status: false,
            push: false,
            pull: false,
            add: None,
            commit: Some("Initial commit".to_string()),
            log: false,
            remote: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_show() {
        let cli = test_cli(Commands::Pipeline {
            list: false,
            show: Some("pipeline-123".to_string()),
            run: None,
            watch: None,
            create: None,
            delete: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_run() {
        let cli = test_cli(Commands::Pipeline {
            list: false,
            show: None,
            run: Some("pipeline-123".to_string()),
            watch: None,
            create: None,
            delete: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_watch() {
        let cli = test_cli(Commands::Pipeline {
            list: false,
            show: None,
            run: None,
            watch: Some("pipeline-123".to_string()),
            create: None,
            delete: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_create() {
        let cli = test_cli(Commands::Pipeline {
            list: false,
            show: None,
            run: None,
            watch: None,
            create: Some("new-pipeline".to_string()),
            delete: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_pipeline_delete() {
        let cli = test_cli(Commands::Pipeline {
            list: false,
            show: None,
            run: None,
            watch: None,
            create: None,
            delete: Some("pipeline-123".to_string()),
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_runner_info() {
        let cli = test_cli(Commands::Runner {
            list: false,
            info: Some("runner-123".to_string()),
            register: None,
            capacity: None,
            deregister: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_runner_register() {
        let cli = test_cli(Commands::Runner {
            list: false,
            info: None,
            register: Some("new-runner".to_string()),
            capacity: None,
            deregister: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_runner_deregister() {
        let cli = test_cli(Commands::Runner {
            list: false,
            info: None,
            register: None,
            capacity: None,
            deregister: Some("runner-123".to_string()),
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_runner_capacity() {
        let cli = test_cli(Commands::Runner {
            list: false,
            info: None,
            register: None,
            capacity: Some(4),
            deregister: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_repo_create() {
        let cli = test_cli(Commands::Repo {
            list: false,
            create: Some("new-repo".to_string()),
            info: None,
            delete: None,
            clone: None,
            init: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_repo_info() {
        let cli = test_cli(Commands::Repo {
            list: false,
            create: None,
            info: Some("repo-123".to_string()),
            delete: None,
            clone: None,
            init: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_repo_delete() {
        let cli = test_cli(Commands::Repo {
            list: false,
            create: None,
            info: None,
            delete: Some("repo-123".to_string()),
            clone: None,
            init: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_repo_clone() {
        let cli = test_cli(Commands::Repo {
            list: false,
            create: None,
            info: None,
            delete: None,
            clone: Some("my-repo".to_string()),
            init: None,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_repo_init() {
        let cli = test_cli(Commands::Repo {
            list: false,
            create: None,
            info: None,
            delete: None,
            clone: None,
            init: Some("/tmp/my-repo".to_string()),
        });
        assert!(run_cli(cli).await.is_ok());
    }

    #[tokio::test]
    async fn test_auth_whoami() {
        let cli = test_cli(Commands::Auth {
            login: None,
            logout: false,
            status: false,
            whoami: true,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    // ─── Commands::Review error-branch tests ──────────────────────────────────

    /// Test-only options for building a Review command.
    /// Only covers the parameters that actually vary across test call sites;
    /// base, target, and context are always None in the existing tests.
    struct ReviewTestOpts {
        staged: bool,
        diff: bool,
        diff_content: Option<String>,
        provider: String,
        verbose: bool,
    }

    impl ReviewTestOpts {}

    fn review_cli(opts: ReviewTestOpts) -> Cli {
        test_cli(Commands::Review {
            staged: opts.staged,
            diff: opts.diff,
            diff_content: opts.diff_content,
            base: None,
            target: None,
            context: None,
            provider: opts.provider,
            verbose: opts.verbose,
        })
    }

    #[tokio::test]
    async fn test_review_unknown_provider_returns_err() {
        // Unknown provider string should cause run_cli to return an error
        let cli = review_cli(ReviewTestOpts {
            staged: true,
            diff: false,
            diff_content: None,
            provider: "not_a_provider".into(),
            verbose: false,
        });
        let result = run_cli(cli).await;
        assert!(result.is_err(), "expected error for unknown provider");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unknown provider") || err_msg.contains("invalid provider"),
            "expected 'Unknown provider' or 'invalid provider' in error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_review_diff_flag_requires_diff_content() {
        // --diff with no --diff-content should error
        let cli = review_cli(ReviewTestOpts {
            staged: false,
            diff: true,
            diff_content: None,
            provider: "anthropic".into(),
            verbose: false,
        });
        let result = run_cli(cli).await;
        assert!(
            result.is_err(),
            "expected error when --diff is set but --diff-content is missing"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("--diff") || err_msg.contains("diff_content"),
            "expected error to mention --diff or diff_content, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_review_empty_diff_returns_ok() {
        // When git diff returns no changes, the CLI prints "No changes to review" and returns Ok
        // We use --diff with empty content to simulate this path
        let cli = review_cli(ReviewTestOpts {
            staged: false,
            diff: true,
            diff_content: Some(String::new()),
            provider: "anthropic".into(),
            verbose: false,
        });
        // This should NOT error — it should print "No changes to review" and return Ok
        let result = run_cli(cli).await;
        // The path checks diff_text.trim().is_empty() and returns Ok if empty
        assert!(result.is_ok(), "expected Ok for empty diff");
    }

    #[tokio::test]
    async fn test_review_whitespace_only_diff_returns_ok() {
        // Whitespace-only diff content is treated as empty
        let cli = review_cli(ReviewTestOpts {
            staged: false,
            diff: true,
            diff_content: Some("   \n\t  ".to_string()),
            provider: "anthropic".into(),
            verbose: false,
        });
        assert!(run_cli(cli).await.is_ok());
    }

    // ─── provider_type_from_name (pure, no network) ────────────────────────────

    #[test]
    fn test_provider_type_from_name_anthropic() {
        use crate::review::provider_type_from_name;
        assert!(provider_type_from_name("anthropic").is_ok());
        assert_eq!(
            provider_type_from_name("anthropic").unwrap(),
            gitforge_ai::ProviderType::Anthropic
        );
    }

    #[test]
    fn test_provider_type_from_name_openai() {
        use crate::review::provider_type_from_name;
        assert!(provider_type_from_name("openai").is_ok());
        assert_eq!(
            provider_type_from_name("openai").unwrap(),
            gitforge_ai::ProviderType::OpenAI
        );
    }

    #[test]
    fn test_provider_type_from_name_ollama() {
        use crate::review::provider_type_from_name;
        assert!(provider_type_from_name("ollama").is_ok());
        assert_eq!(
            provider_type_from_name("ollama").unwrap(),
            gitforge_ai::ProviderType::Ollama
        );
    }

    #[test]
    fn test_provider_type_from_name_unknown() {
        use crate::review::provider_type_from_name;
        let result = provider_type_from_name("not_a_provider");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown provider"), "got: {}", err);
        assert!(err.contains("not_a_provider"), "got: {}", err);
        assert!(err.contains("anthropic"), "got: {}", err);
        assert!(err.contains("openai"), "got: {}", err);
        assert!(err.contains("ollama"), "got: {}", err);
    }
}
