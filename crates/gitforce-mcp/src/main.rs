//! GitForge MCP Server Binary
//!
//! Entry point for the GitForge MCP server — exposes CI/CD tools to Claude Code CLI.
//!
//! Usage (stdio mode — for Claude Code CLI):
//!   gitforge-mcp               # Runs as MCP server (stdio) — default
//!   gitforge-mcp serve        # Explicit stdio mode
//!
//! Usage (HTTP mode — for LM Studio and HTTP-based MCP clients):
//!   gitforge-mcp http        # Runs HTTP server on port 8080
//!   gitforge-mcp http --port 3000

use clap::Parser;
use std::process;
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
    /// HTTP server port (when command = http)
    #[arg(long, default_value = "8080")]
    port: u16,
}

#[tokio::main]
async fn main() {
    // Initialize logging to stderr (stdout is reserved for JSON-RPC in stdio mode)
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
            // Stdio MCP mode — for Claude Code CLI
            if let Err(e) = gitforce_mcp::McpServer::run() {
                eprintln!("MCP server error: {}", e);
                process::exit(1);
            }
        }
        "http" => {
            // HTTP MCP mode — for LM Studio and HTTP-based clients
            if let Err(e) = gitforce_mcp::http_server::run_http(cli.port).await {
                eprintln!("HTTP server error: {}", e);
                process::exit(1);
            }
        }
        _ => {
            eprintln!("Unknown command: {}", cli.command);
            eprintln!("Usage: gitforge-mcp [serve|http]");
            process::exit(1);
        }
    }
}
