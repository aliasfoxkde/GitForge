//! GitForge MCP Server Binary
//!
//! Entry point for the GitForge MCP server — exposes CI/CD tools to Claude Code CLI.
//!
//! Usage:
//!   gitforge-mcp                    # Run as MCP server (stdio)
//!   gitforge-mcp --help            # Show help
//!   gitforge-mcp ci_run --repo owner/repo --branch main  # Direct command

use clap::Parser;
use gitforce_mcp::McpServer;
use std::process;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "gitforge-mcp")]
#[command(author = "GitForge Team")]
#[command(version = "0.1.0")]
#[command(about = "GitForge MCP server — CI/CD tools for Claude Code CLI")]
enum Args {
    /// Run as MCP server (stdio transport)
    Serve,
    /// Run a single tool directly
    Run {
        /// Tool name
        name: String,
        /// Tool arguments as JSON
        #[arg(last = true)]
        args: Vec<String>,
    },
}

fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args = Args::parse();

    match args {
        Args::Serve => {
            info!("Starting GitForge MCP server");
            if let Err(e) = McpServer::run() {
                eprintln!("MCP server error: {}", e);
                process::exit(1);
            }
        }
        Args::Run { name: _, args: _ } => {
            eprintln!("Direct tool execution not yet implemented");
            eprintln!("Run as MCP server: gitforge-mcp serve");
            process::exit(1);
        }
    }
}
