//! Web Dashboard for GitForge
//!
//! Simple HTML dashboard served at the root path.

use axum::{
    response::{Html, IntoResponse},
    routing::get,
    Router,
};

/// Dashboard HTML
const DASHBOARD_HTML: &str = r#"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>GitForge Dashboard</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0d1117; color: #c9d1d9; min-height: 100vh; }
        .container { max-width: 1200px; margin: 0 auto; padding: 2rem; }
        header { border-bottom: 1px solid #30363d; padding-bottom: 1rem; margin-bottom: 2rem; }
        h1 { color: #58a6ff; font-size: 2rem; }
        .subtitle { color: #8b949e; margin-top: 0.5rem; }
        .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 1.5rem; }
        .card { background: #161b22; border: 1px solid #30363d; border-radius: 6px; padding: 1.5rem; }
        .card h2 { color: #58a6ff; font-size: 1.25rem; margin-bottom: 1rem; display: flex; align-items: center; gap: 0.5rem; }
        .card p { color: #8b949e; line-height: 1.6; margin-bottom: 1rem; }
        .badge { display: inline-block; padding: 0.25rem 0.5rem; border-radius: 12px; font-size: 0.75rem; font-weight: 600; }
        .badge-success { background: rgba(46, 160, 67, 0.2); color: #3fb950; }
        .badge-info { background: rgba(56, 139, 253, 0.2); color: #58a6ff; }
        .badge-warning { background: rgba(210, 153, 34, 0.2); color: #d29922; }
        .metric { display: flex; justify-content: space-between; align-items: center; padding: 0.75rem 0; border-bottom: 1px solid #30363d; }
        .metric:last-child { border-bottom: none; }
        .metric-label { color: #8b949e; }
        .metric-value { font-weight: 600; color: #c9d1d9; }
        .api-section { margin-top: 2rem; }
        .api-endpoint { background: #0d1117; padding: 1rem; border-radius: 6px; margin: 0.5rem 0; font-family: monospace; font-size: 0.9rem; }
        .method { display: inline-block; padding: 0.2rem 0.5rem; border-radius: 4px; font-size: 0.75rem; font-weight: 700; margin-right: 0.5rem; }
        .get { background: rgba(46, 160, 67, 0.3); color: #3fb950; }
        .post { background: rgba(56, 139, 253, 0.3); color: #58a6ff; }
        .delete { background: rgba(248, 81, 73, 0.3); color: #f85149; }
        nav { display: flex; gap: 1rem; margin-top: 1rem; }
        nav a { color: #58a6ff; text-decoration: none; padding: 0.5rem 1rem; border-radius: 6px; transition: background 0.2s; }
        nav a:hover { background: rgba(56, 139, 253, 0.1); }
        .status-indicator { width: 8px; height: 8px; border-radius: 50%; display: inline-block; }
        .online { background: #3fb950; box-shadow: 0 0 8px #3fb950; }
        footer { margin-top: 3rem; padding-top: 1rem; border-top: 1px solid #30363d; color: #8b949e; font-size: 0.875rem; text-align: center; }
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>🚀 GitForge</h1>
            <p class="subtitle">Self-hosted Git platform with CI/CD capabilities</p>
            <nav>
                <a href="/dashboard">Dashboard</a>
                <a href="/health">Health</a>
                <a href="/metrics">Metrics</a>
                <a href="/swagger-ui">API Docs</a>
            </nav>
        </header>

        <div class="grid">
            <div class="card">
                <h2>📊 System Status</h2>
                <div class="metric">
                    <span class="metric-label">API Server</span>
                    <span class="metric-value"><span class="status-indicator online"></span> Online</span>
                </div>
                <div class="metric">
                    <span class="metric-label">Database</span>
                    <span class="metric-value"><span class="badge badge-success">Connected</span></span>
                </div>
                <div class="metric">
                    <span class="metric-label">Version</span>
                    <span class="metric-value">0.2.0</span>
                </div>
            </div>

            <div class="card">
                <h2>🔧 Services</h2>
                <div class="metric">
                    <span class="metric-label">Git Server</span>
                    <span class="badge badge-info">Port 2222/8082</span>
                </div>
                <div class="metric">
                    <span class="metric-label">CI Orchestrator</span>
                    <span class="badge badge-success">Running</span>
                </div>
                <div class="metric">
                    <span class="metric-label">Runner Agents</span>
                    <span class="metric-value">0 online</span>
                </div>
            </div>

            <div class="card">
                <h2>📦 Resources</h2>
                <div class="metric">
                    <span class="metric-label">Repositories</span>
                    <span class="metric-value">0</span>
                </div>
                <div class="metric">
                    <span class="metric-label">Pipelines</span>
                    <span class="metric-value">0</span>
                </div>
                <div class="metric">
                    <span class="metric-label">Artifacts</span>
                    <span class="metric-value">0</span>
                </div>
            </div>

            <div class="card">
                <h2>⚡ CI/CD</h2>
                <div class="metric">
                    <span class="metric-label">Pipeline Runs (24h)</span>
                    <span class="metric-value">0</span>
                </div>
                <div class="metric">
                    <span class="metric-label">Success Rate</span>
                    <span class="metric-value">--%</span>
                </div>
                <div class="metric">
                    <span class="metric-label">Avg Duration</span>
                    <span class="metric-value">--</span>
                </div>
            </div>
        </div>

        <div class="card api-section">
            <h2>🔌 API Endpoints</h2>
            <div class="api-endpoint">
                <span class="method get">GET</span> /health - Health check
            </div>
            <div class="api-endpoint">
                <span class="method get">GET</span> /metrics - Prometheus metrics
            </div>
            <div class="api-endpoint">
                <span class="method get">GET</span> /api/repos - List repositories
            </div>
            <div class="api-endpoint">
                <span class="method post">POST</span> /api/repos - Create repository
            </div>
            <div class="api-endpoint">
                <span class="method get">GET</span> /api/pipelines - List pipelines
            </div>
            <div class="api-endpoint">
                <span class="method get">GET</span> /api/runners - List runners
            </div>
            <div class="api-endpoint">
                <span class="method get">GET</span> /api/artifacts - List artifacts
            </div>
            <p style="margin-top: 1rem; color: #8b949e;">
                Full API documentation available at <a href="/swagger-ui" style="color: #58a6ff;">/swagger-ui</a>
            </p>
        </div>

        <footer>
            <p>GitForge v0.2.0 • Built with Rust + Axum</p>
            <p style="margin-top: 0.5rem;">
                <a href="/api-docs/openapi.json" style="color: #58a6ff;">OpenAPI Spec</a> •
                <a href="https://github.com/aliasfoxkde/GitForge" style="color: #58a6ff;">GitHub</a>
            </p>
        </footer>
    </div>
</body>
</html>
"#;

/// Dashboard handler
pub async fn dashboard() -> impl IntoResponse {
    Html(DASHBOARD_HTML)
}

/// Create dashboard routes
pub fn dashboard_routes() -> Router {
    Router::new().route("/dashboard", get(dashboard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dashboard_returns_html() {
        let response = dashboard().await.into_response();
        assert_eq!(response.status(), 200);
    }
}
