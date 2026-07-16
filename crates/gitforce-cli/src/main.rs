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
#[command(name = "gitforge")]
#[command(version = "0.1.0")]
#[command(about = "GitForge CLI - Local-first Git platform client")]
struct Cli {
    #[arg(short, long)]
    verbose: bool,
    #[arg(short, long)]
    server: Option<String>,
    #[arg(short, long)]
    token: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Authentication commands
    Auth {
        #[arg(short, long)]
        login: Option<String>,
        #[arg(short, long)]
        logout: bool,
        #[arg(short, long)]
        status: bool,
    },
    /// Repository commands
    Repo {
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        create: Option<String>,
        #[arg(short, long)]
        info: Option<String>,
        #[arg(short, long)]
        delete: Option<String>,
    },
    /// Pipeline commands
    Pipeline {
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        show: Option<String>,
        #[arg(short, long)]
        run: Option<String>,
        #[arg(short, long)]
        watch: Option<String>,
    },
    /// Runner commands
    Runner {
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        info: Option<String>,
        #[arg(short, long)]
        register: Option<String>,
        #[arg(short, long)]
        capacity: Option<i32>,
    },
    /// Sync commands
    Sync {
        #[arg(short, long)]
        status: bool,
        #[arg(short, long)]
        push: bool,
        #[arg(short, long)]
        pull: bool,
        #[arg(short, long)]
        init: Option<String>,
    },
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

    let config = Config::load().unwrap_or_default();
    let server = cli.server.unwrap_or_else(|| config.server_url.clone());

    match &cli.command {
        Commands::Auth { login, logout, status } => {
            if let Some(username) = login {
                tracing::info!("Logging in as {} to {}", username, server);
                println!("Login successful! (API not yet implemented)");
            } else if *logout {
                tracing::info!("Logged out");
            } else if *status {
                if config.token.is_some() {
                    println!("Authenticated to {}", config.server_url);
                } else {
                    println!("Not authenticated. Run `gitforge auth login` to authenticate.");
                }
            }
        }
        Commands::Repo { list, create, info, delete } => {
            if *list {
                println!("Repositories:\n  (API not yet wired)");
            } else if let Some(name) = create {
                tracing::info!("Creating repository {}", name);
                println!("Repository '{}' created (API not yet wired)", name);
            } else if let Some(name) = info {
                println!("Repository '{}' info (API not yet wired)", name);
            } else if let Some(name) = delete {
                tracing::warn!("Deleting repository {}", name);
                println!("Repository '{}' deleted (API not yet wired)", name);
            }
        }
        Commands::Pipeline { list, show, run, watch } => {
            if *list {
                println!("Pipelines:\n  (API not yet wired)");
            } else if let Some(id) = show {
                println!("Pipeline {} (API not yet wired)", id);
            } else if let Some(id) = run {
                println!("Pipeline {} triggered (API not yet wired)", id);
            } else if let Some(id) = watch {
                println!("Watching pipeline {} (not yet implemented)", id);
            }
        }
        Commands::Runner { list, info, register, capacity } => {
            if *list {
                println!("Runners:\n  (API not yet wired)");
            } else if let Some(id) = info {
                println!("Runner {} (API not yet wired)", id);
            } else if let Some(name) = register {
                let cap = capacity.unwrap_or(2);
                println!("Runner '{}' registered with capacity {} (API not yet wired)", name, cap);
            }
        }
        Commands::Sync { status, push, pull, init } => {
            let local_dir = config.local_data_dir.clone();
            let sync_client = sync::SyncClient::new(local_dir.clone());

            if let Err(e) = sync_client.init().await {
                tracing::warn!("Failed to initialize sync: {}", e);
            }

            if *status {
                let sync_status = sync_client.status().await;
                println!("Local storage: {}", local_dir.display());
                println!("Sync status: {:?}", sync_status);
            } else if *push {
                if let Some(token) = &config.token {
                    match sync_client.push(&config.api_url(), token).await {
                        Ok(response) => {
                            println!("Push successful!");
                            println!("  Remote revision: {}", response.remote_rev);
                            if response.conflicts.is_empty() {
                                println!("  No conflicts");
                            } else {
                                println!("  Conflicts: {}", response.conflicts.join(", "));
                            }
                        }
                        Err(e) => {
                            tracing::error!("Push failed: {}", e);
                            println!("Push failed: {}", e);
                        }
                    }
                } else {
                    println!("Not authenticated. Run `gitforge auth login` first.");
                }
            } else if *pull {
                if let Some(token) = &config.token {
                    match sync_client.pull(&config.api_url(), token).await {
                        Ok(response) => {
                            println!("Pull successful!");
                            println!("  Remote revision: {}", response.remote_rev);
                            println!("  Repositories synced: {}", response.repos.len());
                            println!("  Pipelines synced: {}", response.pipelines.len());
                        }
                        Err(e) => {
                            tracing::error!("Pull failed: {}", e);
                            println!("Pull failed: {}", e);
                        }
                    }
                } else {
                    println!("Not authenticated. Run `gitforge auth login` first.");
                }
            } else if let Some(directory) = init {
                let path = PathBuf::from(directory);
                if path.exists() {
                    anyhow::bail!("Directory {} already exists", directory);
                }
                std::fs::create_dir_all(&path)?;
                let init_client = sync::SyncClient::new(path.clone());
                init_client.init().await?;
                println!("Initialized local storage at {}", path.display());
            }
        }
    }
    Ok(())
}

