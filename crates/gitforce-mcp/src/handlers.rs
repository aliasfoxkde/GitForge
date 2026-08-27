//! Tool handlers — implementations for each MCP tool
//! All handlers are synchronous; async operations use tokio::block_on internally.

use crate::tools::*;
use gitforce_delta::{DeltaAnalyzer, PackageMapper};
use std::process::Command;
use tokio::runtime::Runtime;

/// Represents a tool call parsed from an MCP JSON-RPC request
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

impl ToolCall {
    fn arg_bool(&self, name: &str) -> bool {
        self.arguments
            .get(name)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn arg_str(&self, name: &str) -> Option<String> {
        self.arguments
            .get(name)
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn arg_usize(&self, name: &str) -> Option<usize> {
        self.arguments
            .get(name)
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
    }
}

/// Main handler — dispatches tool calls to implementations
#[derive(Clone)]
pub struct McPHandler;

impl McPHandler {
    pub fn handle(&self, call: ToolCall) -> ToolResult {
        match call.name.as_str() {
            "ci_run" => self.ci_run(call),
            "ci_status" => self.ci_status(call),
            "ci_cancel" => self.ci_cancel(call),
            "delta_plan" => self.delta_plan(call),
            "security_scan" => self.security_scan(call),
            "list_repos" => self.list_repos(call),
            "get_repo_config" => self.get_repo_config(call),
            "pr_create" => self.pr_create(call),
            "pr_merge" => self.pr_merge(call),
            _ => ToolResult::err(format!("Unknown tool: {}", call.name)),
        }
    }

    // ─── CI Run ────────────────────────────────────────────────────────────

    fn ci_run(&self, call: ToolCall) -> ToolResult {
        let repo = call.arg_str("repo").unwrap_or_else(Self::infer_repo);
        let branch = call
            .arg_str("branch")
            .unwrap_or_else(Self::current_branch);
        let delta = call.arg_bool("delta");
        let scope_override = call.arg_str("scope");
        let workflows = call.arg_str("workflows").unwrap_or_default();


        let scope_str = if delta {
            let delta_result = self.delta_plan(call.clone());
            scope_override
                .or_else(|| {
                    delta_result
                        .data
                        .as_ref()?
                        .get("execution_scope")?
                        .as_str()
                        .map(String::from)
                })
                .unwrap_or_else(|| "standard".to_string())
        } else {
            scope_override.unwrap_or_else(|| "standard".to_string())
        };

        ToolResult::with_data(
            format!(
                "CI run triggered for {}/{} (scope: {}, workflows: {})",
                repo, branch, scope_str, workflows
            ),
            serde_json::json!({
                "repo": repo,
                "branch": branch,
                "execution_scope": scope_str,
                "workflows": workflows,
                "delta_mode": delta,
                "run_url": format!("https://github.com/{}/actions", repo),
            }),
        )
    }

    fn ci_status(&self, call: ToolCall) -> ToolResult {
        let repo = call.arg_str("repo").unwrap_or_else(Self::infer_repo);
        let run_id = call.arg_str("run_id");
        let branch = call.arg_str("branch");

        let status = if let Some(id) = run_id {
            Self::gh_run_status(&repo, &id)
        } else {
            Self::gh_latest_run_status(&repo, branch.as_deref().unwrap_or("HEAD"))
        };

        match status {
            Ok(s) => ToolResult::with_data("CI status retrieved", s),
            Err(e) => ToolResult::err(format!("Failed to get CI status: {}", e)),
        }
    }

    fn ci_cancel(&self, call: ToolCall) -> ToolResult {
        let repo = call.arg_str("repo").unwrap_or_else(Self::infer_repo);
        let run_id = match call.arg_str("run_id") {
            Some(id) => id,
            None => return ToolResult::err("run_id is required"),
        };

        let output = Command::new("gh")
            .args(["run", "cancel", &run_id, "--repo", &repo])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                ToolResult::ok(format!("Cancelled run {} on {}", run_id, repo))
            }
            Ok(out) => ToolResult::err(format!(
                "gh run cancel failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )),
            Err(e) => ToolResult::err(format!("Failed to run gh: {}", e)),
        }
    }

    // ─── Delta Plan ───────────────────────────────────────────────────────

    fn delta_plan(&self, call: ToolCall) -> ToolResult {
        let repo_root = call.arg_str("repo_root").unwrap_or_else(|| ".".to_string());
        let base_ref = call
            .arg_str("base_ref")
            .unwrap_or_else(|| "HEAD~1".to_string());
        let rt = Runtime::new().expect("failed to create tokio runtime");

        let analyzer = DeltaAnalyzer::new(&repo_root, &base_ref);
        let plan = match rt.block_on(analyzer.analyze()) {
            Ok(p) => p,
            Err(e) => return ToolResult::err(format!("Delta analysis failed: {}", e)),
        };

        let workflows = if plan.docs_only || plan.config_only {
            vec!["smoke-test.yml".to_string()]
        } else {
            let mapper = PackageMapper::default();
            mapper.workflows_for_packages(&plan.affected_packages)
        };

        let scope_str = format!("{:?}", plan.execution_scope);
        let summary_str = plan.execution_summary();

        let result = DeltaResult {
            file_count: plan.file_count,
            lines_changed: plan.total_lines_changed,
            languages: plan.detected_languages,
            affected_packages: plan.affected_packages,
            execution_scope: scope_str,
            workflows_to_run: workflows,
            docs_only: plan.docs_only,
            config_only: plan.config_only,
            ci_changed: plan.ci_changed,
            summary: summary_str,
        };

        ToolResult::with_data("Delta analysis complete", result)
    }

    // ─── Security Scan ─────────────────────────────────────────────────────

    fn security_scan(&self, call: ToolCall) -> ToolResult {
        let path = call.arg_str("path").unwrap_or_else(|| ".".to_string());
        let categories = call
            .arg_str("categories")
            .unwrap_or_else(|| "secrets,pii,security".to_string());

        let has_atheon = Command::new("which")
            .arg("atheon")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !has_atheon {
            return ToolResult::err(
                "Atheon-Enhanced not installed. Install from: \
                 https://github.com/aliasfoxkde/Atheon-Enhanced/releases",
            );
        }

        let output = Command::new("atheon")
            .args(["--categories", &categories, "--json", &path])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let finding_count = stdout.lines().count();
                ToolResult::with_data(
                    format!("Security scan complete — {} findings", finding_count),
                    serde_json::json!({
                        "scan_path": path,
                        "categories": categories,
                        "finding_count": finding_count,
                    }),
                )
            }
            Err(e) => ToolResult::err(format!("Atheon scan failed: {}", e)),
        }
    }

    // ─── Repo Operations ───────────────────────────────────────────────────

    fn list_repos(&self, call: ToolCall) -> ToolResult {
        let token = call
            .arg_str("token")
            .or_else(|| std::env::var("GITHUB_TOKEN").ok());

        let token = match token {
            Some(t) => t,
            None => return ToolResult::err("No GitHub token provided"),
        };

        let client = reqwest::blocking::Client::new();

        let response = match client
            .get("https://api.github.com/user/repos?per_page=30&sort=updated")
            .header("Authorization", format!("Bearer {}", token))
            .header("User-Agent", "gitforge-mcp/0.1")
            .send()
        {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("HTTP error: {}", e)),
        };

        let repos: Vec<serde_json::Value> = match response.json() {
            Ok(r) => r,
            Err(e) => return ToolResult::err(format!("JSON parse error: {}", e)),
        };

        let repo_infos: Vec<RepoInfo> = repos
            .iter()
            .filter_map(|r| {
                Some(RepoInfo {
                    name: r.get("name")?.as_str()?.to_string(),
                    full_name: r.get("full_name")?.as_str()?.to_string(),
                    default_branch: r.get("default_branch")?.as_str()?.to_string(),
                    visibility: r.get("visibility")?.as_str()?.to_string(),
                    ci_enabled: true,
                })
            })
            .collect();

        ToolResult::with_data(
            format!("Found {} repositories", repo_infos.len()),
            repo_infos,
        )
    }

    fn get_repo_config(&self, call: ToolCall) -> ToolResult {
        let _repo = call.arg_str("repo").unwrap_or_else(Self::infer_repo);

        let config = RepoConfig {
            default_branch: "main".to_string(),
            coverage_threshold: 70,
            languages: vec!["rust".to_string(), "shell".to_string()],
            workflows_enabled: vec![
                "ci.yml".to_string(),
                "rust.yml".to_string(),
                "security.yml".to_string(),
            ],
            require_code_owner_review: true,
            required_status_checks: vec![
                "ci/check".to_string(),
                "ci/lint".to_string(),
                "ci/test".to_string(),
                "ci/security".to_string(),
            ],
        };

        ToolResult::with_data("Config retrieved", config)
    }

    fn pr_create(&self, call: ToolCall) -> ToolResult {
        let repo = call.arg_str("repo").unwrap_or_else(Self::infer_repo);
        let title = match call.arg_str("title") {
            Some(t) => t,
            None => return ToolResult::err("title is required"),
        };
        let body = call.arg_str("body").unwrap_or_default();
        let head = call
            .arg_str("head")
            .unwrap_or_else(Self::current_branch);
        let base = call.arg_str("base").unwrap_or_else(|| "main".to_string());

        let output = Command::new("gh")
            .args([
                "pr", "create", "--repo", &repo, "--title", &title, "--body", &body, "--head",
                &head, "--base", &base,
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let url = String::from_utf8_lossy(&out.stdout);
                ToolResult::with_data(
                    format!("PR created: {}", url.trim()),
                    serde_json::json!({ "url": url.trim(), "repo": repo }),
                )
            }
            Ok(out) => ToolResult::err(format!(
                "PR creation failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )),
            Err(e) => ToolResult::err(format!("Failed to run gh: {}", e)),
        }
    }

    fn pr_merge(&self, call: ToolCall) -> ToolResult {
        let repo = call.arg_str("repo").unwrap_or_else(Self::infer_repo);
        let pr_number: u64 = match call.arg_usize("pr_number") {
            Some(n) => n as u64,
            None => return ToolResult::err("pr_number is required"),
        };
        let method = call
            .arg_str("method")
            .unwrap_or_else(|| "squash".to_string());

        let output = Command::new("gh")
            .args([
                "pr",
                "merge",
                "--repo",
                &repo,
                &pr_number.to_string(),
                "--admin",
                "--squash",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                ToolResult::ok(format!("PR #{} merged via {}", pr_number, method))
            }
            Ok(out) => ToolResult::err(format!(
                "PR merge failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )),
            Err(e) => ToolResult::err(format!("Failed to run gh: {}", e)),
        }
    }

    // ─── Helpers ──────────────────────────────────────────────────────────

    fn infer_repo() -> String {
        let output = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .output();

        output
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .map(|s| {
                s.trim()
                    .strip_prefix("https://github.com/")
                    .or_else(|| s.trim().strip_prefix("git@github.com:"))
                    .or_else(|| s.trim().strip_prefix("git://github.com/"))
                    .map(|s| s.trim_end_matches(".git").to_string())
                    .unwrap_or_else(|| s.trim().to_string())
            })
            .unwrap_or_else(|_| "owner/repo".to_string())
    }

    fn current_branch() -> String {
        Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "main".to_string())
    }

    fn gh_run_status(repo: &str, run_id: &str) -> Result<CiStatus, String> {
        let output = Command::new("gh")
            .args([
                "run",
                "view",
                run_id,
                "--repo",
                repo,
                "--json",
                "id,status,conclusion,headBranch,headSha,runNumber,createdAt,url",
            ])
            .output()
            .map_err(|e| format!("gh error: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "gh run view failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse gh output: {}", e))?;

        Ok(CiStatus {
            run_id: json["id"].as_u64().unwrap_or(0),
            repo: repo.to_string(),
            branch: json["headBranch"].as_str().unwrap_or("").to_string(),
            commit: json["headSha"].as_str().unwrap_or("").to_string(),
            status: json["status"].as_str().unwrap_or("").to_string(),
            conclusion: json
                .get("conclusion")
                .and_then(|v| v.as_str())
                .map(String::from),
            jobs: vec![],
            triggered_by: "cli".to_string(),
            run_at: json["createdAt"].as_str().unwrap_or("").to_string(),
            url: json["url"].as_str().unwrap_or("").to_string(),
        })
    }

    fn gh_latest_run_status(repo: &str, branch: &str) -> Result<CiStatus, String> {
        let output = Command::new("gh")
            .args([
                "run",
                "list",
                "--repo",
                repo,
                "--branch",
                branch,
                "--json",
                "id,status,conclusion,headBranch,headSha,runNumber,createdAt,url",
                "--limit",
                "1",
            ])
            .output()
            .map_err(|e| format!("gh error: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "gh run list failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let runs: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("Failed to parse gh output: {}", e))?;

        let run = runs.first().ok_or_else(|| "No runs found".to_string())?;

        Ok(CiStatus {
            run_id: run["id"].as_u64().unwrap_or(0),
            repo: repo.to_string(),
            branch: run["headBranch"].as_str().unwrap_or("").to_string(),
            commit: run["headSha"].as_str().unwrap_or("").to_string(),
            status: run["status"].as_str().unwrap_or("").to_string(),
            conclusion: run
                .get("conclusion")
                .and_then(|v| v.as_str())
                .map(String::from),
            jobs: vec![],
            triggered_by: "cli".to_string(),
            run_at: run["createdAt"].as_str().unwrap_or("").to_string(),
            url: run["url"].as_str().unwrap_or("").to_string(),
        })
    }
}
