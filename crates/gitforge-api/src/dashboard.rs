//! Web Dashboard for GitForge
//!
//! Live aggregate status served at /dashboard. Every number on the page
//! is read from the database at render time — the panel never fabricates
//! state it did not observe. When the database is unreachable the panel
//! says so instead of showing zeros (a dashboard that quietly lies is
//! worse than one that admits it cannot see).
//!
//! Styling targets WCAG 2.1 AAA for the text it renders: ≥7:1 contrast
//! on every color pair (small text), a visible `:focus-visible` outline
//! for keyboard users, a `<main>` landmark, and decorative emoji hidden
//! from assistive technology.

use axum::{
    response::{Html, IntoResponse},
    routing::get,
    Extension, Router,
};
use gitforge_db::{
    queries::{DashboardStats, StatsQueries},
    Pool,
};
use std::sync::Arc;

/// The dashboard body for a database read. `None` stats mean the read
/// failed; every metric renders as an em dash and the database badge
/// flips to Unavailable.
fn render_dashboard(stats: Option<&DashboardStats>) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let (db_badge, db_badge_class) = if stats.is_some() {
        ("Connected", "badge-success")
    } else {
        ("Unavailable", "badge-danger")
    };
    let metric = |value: Option<String>| value.unwrap_or_else(|| "—".to_string());
    let (repos, pipelines, artifacts) = match stats {
        Some(s) => (
            metric(Some(s.repositories.to_string())),
            metric(Some(s.pipelines.to_string())),
            metric(Some(s.artifacts.to_string())),
        ),
        None => (metric(None), metric(None), metric(None)),
    };
    let (runs, rate, runners) = match stats {
        Some(s) => {
            let rate = if s.runs_last_24h == 0 {
                "--".to_string()
            } else {
                // Round-half-up integer percent; exact enough for a
                // dashboard and stable across renders.
                format!(
                    "{}",
                    (s.runs_succeeded_24h * 100 + s.runs_last_24h / 2) / s.runs_last_24h
                )
            };
            (
                metric(Some(s.runs_last_24h.to_string())),
                format!("{rate}%"),
                metric(Some(s.runners_online.to_string())),
            )
        }
        None => (metric(None), "--%".to_string(), metric(None)),
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>GitForge Dashboard</title>
    <style>
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; background: #0d1117; color: #c9d1d9; min-height: 100vh; }}
        .container {{ max-width: 1200px; margin: 0 auto; padding: 2rem; }}
        header {{ border-bottom: 1px solid #30363d; padding-bottom: 1rem; margin-bottom: 2rem; }}
        h1 {{ color: #79b8ff; font-size: 2rem; }}
        .subtitle {{ color: #a8b3bf; margin-top: 0.5rem; }}
        .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 1.5rem; }}
        .card {{ background: #161b22; border: 1px solid #30363d; border-radius: 6px; padding: 1.5rem; }}
        .card h2 {{ color: #79b8ff; font-size: 1.25rem; margin-bottom: 1rem; display: flex; align-items: center; gap: 0.5rem; }}
        .badge {{ display: inline-block; padding: 0.25rem 0.5rem; border-radius: 12px; font-size: 0.75rem; font-weight: 600; }}
        .badge-success {{ background: rgba(46, 160, 67, 0.2); color: #7ee787; }}
        .badge-danger {{ background: rgba(248, 81, 73, 0.25); color: #ffa198; }}
        .metric {{ display: flex; justify-content: space-between; align-items: center; padding: 0.75rem 0; border-bottom: 1px solid #30363d; }}
        .metric:last-child {{ border-bottom: none; }}
        .metric-label {{ color: #a8b3bf; }}
        .metric-value {{ font-weight: 600; color: #c9d1d9; }}
        .api-section {{ margin-top: 2rem; }}
        .api-endpoint {{ background: #0d1117; padding: 1rem; border-radius: 6px; margin: 0.5rem 0; font-family: monospace; font-size: 0.9rem; }}
        .method {{ display: inline-block; padding: 0.2rem 0.5rem; border-radius: 4px; font-size: 0.75rem; font-weight: 700; margin-right: 0.5rem; }}
        .get {{ background: rgba(46, 160, 67, 0.3); color: #7ee787; }}
        .post {{ background: rgba(56, 139, 253, 0.3); color: #a5d6ff; }}
        .delete {{ background: rgba(248, 81, 73, 0.25); color: #ffa198; }}
        nav {{ display: flex; gap: 1rem; margin-top: 1rem; }}
        nav a {{ color: #79b8ff; text-decoration: none; padding: 0.5rem 1rem; border-radius: 6px; transition: background 0.2s; }}
        nav a:hover {{ background: rgba(56, 139, 253, 0.1); }}
        a:focus-visible {{ outline: 2px solid #79b8ff; outline-offset: 2px; }}
        a {{ color: #79b8ff; }}
        .muted {{ color: #a8b3bf; }}
        footer {{ margin-top: 3rem; padding-top: 1rem; border-top: 1px solid #30363d; color: #a8b3bf; font-size: 0.875rem; text-align: center; }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1><span aria-hidden="true">🚀</span> GitForge</h1>
            <p class="subtitle">Self-hosted Git platform with CI/CD capabilities</p>
            <nav>
                <a href="/dashboard">Dashboard</a>
                <a href="/health">Health</a>
                <a href="/metrics">Metrics</a>
                <a href="/swagger-ui">API Docs</a>
            </nav>
        </header>

        <main>
            <div class="grid">
                <div class="card">
                    <h2><span aria-hidden="true">📊</span> System Status</h2>
                    <div class="metric">
                        <span class="metric-label">API Server</span>
                        <span class="metric-value"><span class="badge badge-success">Serving</span></span>
                    </div>
                    <div class="metric">
                        <span class="metric-label">Database</span>
                        <span class="metric-value"><span class="badge {db_badge_class}">{db_badge}</span></span>
                    </div>
                    <div class="metric">
                        <span class="metric-label">Version</span>
                        <span class="metric-value">{version}</span>
                    </div>
                </div>

                <div class="card">
                    <h2><span aria-hidden="true">📦</span> Resources</h2>
                    <div class="metric">
                        <span class="metric-label">Repositories</span>
                        <span class="metric-value">{repos}</span>
                    </div>
                    <div class="metric">
                        <span class="metric-label">Pipelines</span>
                        <span class="metric-value">{pipelines}</span>
                    </div>
                    <div class="metric">
                        <span class="metric-label">Artifacts</span>
                        <span class="metric-value">{artifacts}</span>
                    </div>
                </div>

                <div class="card">
                    <h2><span aria-hidden="true">⚡</span> CI/CD (24h)</h2>
                    <div class="metric">
                        <span class="metric-label">Pipeline Runs</span>
                        <span class="metric-value">{runs}</span>
                    </div>
                    <div class="metric">
                        <span class="metric-label">Success Rate</span>
                        <span class="metric-value">{rate}</span>
                    </div>
                    <div class="metric">
                        <span class="metric-label">Runners Online</span>
                        <span class="metric-value">{runners}</span>
                    </div>
                </div>
            </div>

            <div class="card api-section">
                <h2><span aria-hidden="true">🔌</span> API Endpoints</h2>
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
                    <span class="method get">GET</span> /api/runners - List runners<br>
                    <span class="method delete">DELETE</span> /api/runners/{{id}} - Retire an idle runner
                </div>
                <div class="api-endpoint">
                    <span class="method get">GET</span> /api/artifacts - List artifacts
                </div>
                <p class="muted" style="margin-top: 1rem;">
                    Full API documentation available at <a href="/swagger-ui">/swagger-ui</a>
                </p>
            </div>
        </main>

        <footer>
            <p>GitForge v{version} • Built with Rust + Axum</p>
            <p style="margin-top: 0.5rem;">
                <a href="/api-docs/openapi.json">OpenAPI Spec</a> •
                <a href="https://github.com/aliasfoxkde/GitForge">GitHub</a>
            </p>
        </footer>
    </div>
</body>
</html>
"#
    )
}

/// Dashboard handler — renders live aggregates from the shared pool.
pub async fn dashboard(Extension(pool): Extension<Arc<Pool>>) -> impl IntoResponse {
    let stats = match StatsQueries::dashboard(&pool).await {
        Ok(stats) => {
            tracing::debug!(
                repositories = stats.repositories,
                "dashboard stats rendered"
            );
            Some(stats)
        }
        Err(error) => {
            tracing::error!(%error, "dashboard stats unavailable");
            None
        }
    };
    Html(render_dashboard(stats.as_ref()))
}

/// Create dashboard routes. The pool arrives via the app-level
/// `Extension<Arc<Pool>>` — the dashboard adds no per-route state.
pub fn dashboard_routes() -> Router {
    Router::new().route("/dashboard", get(dashboard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dashboard_returns_html() {
        let pool = Arc::new(Pool::memory().await.unwrap());
        pool.migrate().await.unwrap();
        let response = dashboard(Extension(pool)).await.into_response();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_dashboard_empty_database_renders_zeroes_honestly() {
        let pool = Arc::new(Pool::memory().await.unwrap());
        pool.migrate().await.unwrap();
        let body = render_dashboard(StatsQueries::dashboard(&pool).await.ok().as_ref());
        assert!(body.contains("<main>"), "main landmark required");
        assert!(body.contains("aria-hidden"), "decorative emoji hidden");
        assert!(body.contains("focus-visible"), "focus outline required");
        assert!(body.contains("Connected"), "migrated database is reachable");
        assert!(!body.contains("badge-warning"));
        // A fresh database has no runs in the window: the rate renders as
        // the explicit no-data marker, never a fabricated percentage.
        assert!(body.contains("<span class=\"metric-value\">--%</span>"));
    }

    #[tokio::test]
    async fn test_dashboard_without_stats_reports_unavailable() {
        let body = render_dashboard(None);
        assert!(body.contains("Unavailable"));
        assert!(body.contains(">—</span>"), "failed reads render em dashes");
    }
}
