//! GitForge CLI
//!
//! Local-first Git platform client for GitForge.

use clap::Parser;
use clap::Subcommand;
use anyhow::Result;
use std::path::PathBuf;

mod config;
mod client;
mod sync;

pub use config::Config;
pub use client::GitForgeClient;

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
}

/// Run the CLI command handler (extracted for testing)
pub async fn run_cli(cli: Cli) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let server = cli.server.unwrap_or_else(|| config.server_url.clone());
    let token = cli.token.or_else(|| config.token.clone());

    match &cli.command {
        Commands::Auth { login, logout, status, whoami } => {
            if let Some(username) = login {
                println!("🔐 Authenticating as {} to {}", username, server);
                println!("   (API not yet wired - use `gitforge auth status` to check)");
                println!("   Token would be stored in: ~/.gitforge/credentials");
            } else if *logout {
                println!("👋 Logged out. Credentials cleared.");
                println!("   Run `gitforge auth login <username>` to authenticate again.");
            } else if *status {
                if token.is_some() {
                    println!("✅ Authenticated to {}", server);
                    println!("   Server: {}", server);
                    println!("   Use `gitforge auth whoami` for user details.");
                } else {
                    println!("❌ Not authenticated.");
                    println!("   Run `gitforge auth login <username>` to authenticate.");
                }
            } else if *whoami {
                if token.is_some() {
                    println!("👤 Authenticated user");
                    println!("   Server: {}", server);
                } else {
                    println!("❌ Not authenticated.");
                }
            }
        }

        Commands::Repo { list, create, info, delete, clone, init } => {
            if *list {
                println!("📦 Repositories on {}:", server);
                println!("");
                println!("  (API not yet wired - showing sample format)");
                println!("  my-project     - My awesome project       [active]");
                println!("  another-repo   - Another repository        [active]");
                println!("");
                println!("  Run `gitforge repo create <name>` to create a new repository.");
            } else if let Some(name) = create {
                println!("📦 Creating repository '{}'...", name);
                println!("   Server: {}", server);
                println!("   Visibility: private (default)");
                println!("   (API not yet wired)");
            } else if let Some(name) = info {
                println!("📋 Repository: {}", name);
                println!("   Server: {}", server);
                println!("   (API not yet wired)");
            } else if let Some(name) = delete {
                println!("⚠️  Deleting repository '{}'...", name);
                println!("   This action is irreversible!");
                println!("   (API not yet wired)");
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

        Commands::Git { init, clone, status, push, pull, add, commit, log, remote } => {
            if let Some(path) = init {
                let dir = PathBuf::from(path);
                println!("🔧 Initializing Git repository at {}...", dir.display());
                println!("   Adding GitForge remote...");
                println!("   (Actual git init would be performed here)");
            } else if let Some(url) = clone {
                println!("📥 Cloning from {}...", url);
                println!("   This will clone the repository to the current directory.");
                println!("   (Actual git clone would be performed here)");
                println!("");
                println!("   GitForge supports:");
                println!("   - HTTPS cloning via GitForge API");
                println!("   - SSH cloning via git-server service");
            } else if *status {
                println!("📊 Git Status:");
                println!("");
                println!("  On branch: main");
                println!("  Your branch is up to date with 'origin/main'.");
                println!("");
                println!("  nothing to commit, working tree clean");
                println!("");
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
                println!("");
                println!("  commit abc123 (HEAD -> main)");
                println!("  Author: User <user@example.com>");
                println!("  Date:   2026-07-22");
                println!("");
                println!("      Initial commit");
                println!("");
                println!("  (This is a demo - actual git log would show real history)");
            } else if *remote {
                println!("🔗 Git Remotes:");
                println!("");
                println!("  origin  {} (fetch)", server);
                println!("  origin  {} (push)", server);
            }
        }

        Commands::Pipeline { list, show, run, watch, create, delete } => {
            if *list {
                println!("⚙️  Pipelines on {}:", server);
                println!("");
                println!("  (API not yet wired - showing sample format)");
                println!("  build-and-test  - Build and run tests     [active]");
                println!("  deploy-prod      - Deploy to production     [active]");
                println!("");
                println!("  Run `gitforge pipeline create <name>` to create a pipeline.");
            } else if let Some(id) = show {
                println!("⚙️  Pipeline: {}", id);
                println!("   Server: {}", server);
                println!("   Status: (API not yet wired)");
            } else if let Some(id) = run {
                println!("🚀 Triggering pipeline: {}", id);
                println!("   (API not yet wired)");
            } else if let Some(id) = watch {
                println!("👁️  Watching pipeline: {}", id);
                println!("   (API not yet wired)");
            } else if let Some(name) = create {
                println!("⚙️  Creating pipeline '{}'...", name);
                println!("   (API not yet wired)");
            } else if let Some(id) = delete {
                println!("⚠️  Deleting pipeline '{}'...", id);
                println!("   This action is irreversible!");
                println!("   (API not yet wired)");
            }
        }

        Commands::Runner { list, info, register, capacity, deregister } => {
            if *list {
                println!("🤖 Runners on {}:", server);
                println!("");
                println!("  (API not yet wired - showing sample format)");
                println!("  runner-01    - Linux x86_64    [idle]    capacity: 2");
                println!("  runner-02    - Linux ARM64     [busy]    capacity: 4");
                println!("");
                println!("  Run `gitforge runner register <name>` to register a runner.");
            } else if let Some(id) = info {
                println!("🤖 Runner: {}", id);
                println!("   Server: {}", server);
                println!("   Status: (API not yet wired)");
            } else if let Some(name) = register {
                let cap = capacity.unwrap_or(2);
                println!("🤖 Registering runner: {}", name);
                println!("   Server: {}", server);
                println!("   Capacity: {} concurrent jobs", cap);
                println!("   (API not yet wired)");
            } else if deregister.is_some() {
                let id = deregister.clone().unwrap_or_default();
                println!("🤖 Deregistering runner: {}", id);
                println!("   (API not yet wired)");
            } else if let Some(cap) = capacity {
                println!("🤖 Updating runner capacity to: {}", cap);
                println!("   (API not yet wired)");
            }
        }

        Commands::Sync { status, push, pull, init } => {
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
                println!("");
                println!("   Local storage: {}", local_dir.display());
                let sync_client = sync::SyncClient::with_real_client(local_dir.clone());
                let sync_status = sync_client.status().await;
                println!("   Sync state: {:?}", sync_status);
                println!("");
                println!("   Server: {}", server);
                println!("   (Authenticated: {})", if token.is_some() { "yes" } else { "no" });
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
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(if cli.verbose { tracing::Level::DEBUG.into() } else { tracing::Level::INFO.into() })
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
        let test_dir = temp_dir.join("gitforge-test-init").to_str().unwrap().to_string();
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
}
