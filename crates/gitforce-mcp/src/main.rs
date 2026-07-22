//! GitForge MCP Server Binary
//!
//! Entry point for the GitForge MCP server — exposes CI/CD tools to Claude Code CLI.
//!
//! Usage:
//!   gitforge-mcp               # Runs as MCP server (stdio) — default
//!   gitforge-mcp serve           # Explicit MCP server mode
//!   gitforge-mcp run <tool>     # Run a single tool directly (future)

use clap::Parser;
use gitforce_mcp::McpServer;
use std::process;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "gitforge-mcp",
    author = "GitForge Team",
    version = "0.1.0",
    about = "GitForge MCP server — CI/CD tools for Claude Code CLI"
)]
struct Cli {
    #[arg(default_value = "serve", hide_default_value = true)]
    command: String,
    /// Tool name (when command = run)
    #[arg(default_value = "")]
    tool: String,
    /// Tool arguments as JSON (when command = run)
    #[arg(default_value = "", hide_default_value = true)]
    args: Vec<String>,
}

fn main() {
    // Initialize logging to stderr (stdout is reserved for JSON-RPC)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command.as_str() {
        "serve" => {
            info!("Starting GitForge MCP server");
            if let Err(e) = McpServer::run() {
                eprintln!("MCP server error: {}", e);
                process::exit(1);
            }
        }
        "run" => {
            eprintln!("Direct tool execution not yet implemented");
            eprintln!("Run as MCP server: gitforge-mcp");
            process::exit(1);
        }
        _ => {
            eprintln!("Unknown command: {}", cli.command);
            eprintln!("Usage: gitforge-mcp [serve|run]");
            process::exit(1);
        }
    }
}
