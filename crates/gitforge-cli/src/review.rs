//! GitForge AI Code Review Module
//!
//! Provides AI-powered code review using configurable providers.

use anyhow::{Context, Result};
use gitforge_ai::{
    AiProviderFactory, FileChange, ProviderConfig, ProviderType, ReviewRequest, ReviewResponse,
    Severity,
};
use gitforge_review::{extract_changes_from_diff, ChangeComplexity, DiffStats};
use std::path::Path;
use std::process::Command;

/// Create a review request from git diff
pub fn create_review_request(
    repo_name: &str,
    branch: &str,
    base_branch: Option<&str>,
    diff_content: &str,
    context: &str,
) -> Result<ReviewRequest> {
    let changes =
        extract_changes_from_diff(diff_content).context("Failed to parse diff content")?;

    if changes.is_empty() {
        anyhow::bail!("No file changes found in diff");
    }

    let mut request = ReviewRequest::new(repo_name, branch, changes);

    if let Some(base) = base_branch {
        request = request.with_base_branch(base);
    }

    if !context.is_empty() {
        request = request.with_context(context);
    }

    Ok(request)
}

/// Get git diff for a repository
pub fn get_git_diff(
    repo_path: &Path,
    base_branch: Option<&str>,
    target: Option<&str>,
) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("diff");

    match (base_branch, target) {
        (Some(base), Some(target)) => {
            cmd.arg(format!("{}...{}", base, target));
        }
        (Some(base), None) => {
            cmd.arg(format!("{}...HEAD", base));
        }
        (None, Some(target)) => {
            cmd.arg(format!("{}...HEAD", target));
        }
        (None, None) => {
            cmd.arg("--staged");
        }
    }

    cmd.current_dir(repo_path);

    let output = cmd.output().context("Failed to run git diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get uncommitted changes
pub fn get_uncommitted_diff(repo_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["diff"])
        .current_dir(repo_path)
        .output()
        .context("Failed to run git diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get the current branch name
pub fn get_current_branch(repo_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_path)
        .output()
        .context("Failed to get current branch")?;

    if !output.status.success() {
        anyhow::bail!("git rev-parse failed");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Convert a provider name string to a ProviderType.
///
/// Returns a stable error for unknown names so callers can present a clear
/// diagnostic without needing to re-implement the name-to-type mapping.
pub fn provider_type_from_name(name: &str) -> Result<ProviderType> {
    match name.to_lowercase().as_str() {
        "anthropic" => Ok(ProviderType::Anthropic),
        "openai" => Ok(ProviderType::OpenAI),
        "ollama" => Ok(ProviderType::Ollama),
        other => anyhow::bail!(
            "Unknown provider '{}'. Use: anthropic, openai, or ollama.",
            other
        ),
    }
}

/// Run a code review using the specified provider
pub async fn run_review(
    provider_type: ProviderType,
    request: &ReviewRequest,
) -> Result<ReviewResponse> {
    let config = match provider_type {
        ProviderType::Anthropic => ProviderConfig::anthropic(),
        ProviderType::OpenAI => ProviderConfig::openai(),
        ProviderType::Ollama => ProviderConfig::ollama("http://localhost:11434"),
    };

    // Create the appropriate provider
    let provider: Box<dyn gitforge_ai::AiProvider> = match provider_type {
        ProviderType::Anthropic => {
            let p = AiProviderFactory::create_anthropic(config)
                .context("Failed to create Anthropic provider")?;
            Box::new(p)
        }
        ProviderType::OpenAI => {
            let p = AiProviderFactory::create_openai(config)
                .context("Failed to create OpenAI provider")?;
            Box::new(p)
        }
        ProviderType::Ollama => {
            let p = AiProviderFactory::create_ollama(config)
                .context("Failed to create Ollama provider")?;
            Box::new(p)
        }
    };

    // Check provider health
    if let Err(e) = provider.health_check().await {
        eprintln!("⚠️  Warning: Provider health check failed: {}", e);
        eprintln!("   Continuing anyway...");
    }

    provider
        .generate_review(request)
        .await
        .context("Review generation failed")
}

/// Format and print review results
pub fn print_review_results(response: &ReviewResponse, verbose: bool) {
    println!();
    println!("═══════════════════════════════════════════════════════════════");
    println!("                    📋 CODE REVIEW RESULTS                      ");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("  Provider: {} ({})", response.provider, response.model);
    println!("  Cost: ${:.4}", response.cost_cents as f32 / 100.0);
    println!("  Tokens: {}", response.tokens_used);
    println!();

    // Overall assessment
    let score_color = if response.overall_score >= 80 {
        "🟢"
    } else if response.overall_score >= 60 {
        "🟡"
    } else {
        "🔴"
    };

    println!(
        "  {} Overall Score: {}/100",
        score_color, response.overall_score
    );
    println!();
    println!("  ─────────────────────────────────────────────────────────────");
    println!("  📝 Summary:");
    println!("  {}", response.summary);
    println!();

    // Findings by severity
    let critical = response.findings_by_severity(Severity::Critical);
    let high = response.findings_by_severity(Severity::High);
    let medium = response.findings_by_severity(Severity::Medium);
    let low = response.findings_by_severity(Severity::Low);
    let info = response.findings_by_severity(Severity::Info);

    if !response.findings.is_empty() {
        println!("  ─────────────────────────────────────────────────────────────");
        println!("  🔍 Findings: {} total", response.findings.len());
        println!();

        let print_findings = |severity: &str, findings: &[&gitforge_ai::ReviewFinding]| {
            if !findings.is_empty() {
                println!(
                    "  {} {} ({}):",
                    severity,
                    findings.len(),
                    match severity {
                        "Critical" => "🚨",
                        "High" => "🔴",
                        "Medium" => "🟡",
                        "Low" => "🟢",
                        _ => "ℹ️",
                    }
                );
                for (i, f) in findings.iter().enumerate() {
                    println!("    {}. {}", i + 1, f.title);
                    println!("       📁 {}", f.file);
                    if let (Some(start), Some(end)) = (f.line_start, f.line_end) {
                        println!("       📍 Lines {}-{}", start, end);
                    } else if let Some(start) = f.line_start {
                        println!("       📍 Line {}", start);
                    }
                    println!("       💬 {}", f.description);
                    if verbose {
                        if let Some(suggestion) = &f.suggestion {
                            println!("       ✨ Suggestion: {}", suggestion);
                        }
                        if let Some(snippet) = &f.code_snippet {
                            println!("       💻 Code: {}", snippet);
                        }
                    }
                }
                println!();
            }
        };

        print_findings("Critical", &critical);
        print_findings("High", &high);
        print_findings("Medium", &medium);
        print_findings("Low", &low);
        print_findings("Info", &info);
    } else {
        println!("  ✅ No findings - your code looks great!");
    }

    println!("═══════════════════════════════════════════════════════════════");
}

/// Print diff statistics
pub fn print_diff_stats(diff: &str) -> Result<()> {
    let stats = DiffStats::from_diff(diff).context("Failed to calculate diff stats")?;

    println!();
    println!("  📊 Change Statistics:");
    println!(
        "     Files: {} changed ({} added, {} modified, {} deleted)",
        stats.files_changed, stats.files_added, stats.files_modified, stats.files_deleted
    );
    println!(
        "     Changes: +{} insertions, -{} deletions",
        stats.insertions, stats.deletions
    );

    Ok(())
}

/// Analyze and print change complexity
pub fn print_complexity(changes: &[FileChange]) {
    let complexity = ChangeComplexity::analyze(changes);

    println!();
    println!("  📈 Complexity Analysis:");
    println!("     Files touched: {}", complexity.files_touched);
    println!("     Total lines: {}", complexity.total_lines);
    println!("     Churn: {}", complexity.churn);
    println!(
        "     Test changes: {}",
        if complexity.has_test_changes {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "     Docs changes: {}",
        if complexity.has_docs_changes {
            "yes"
        } else {
            "no"
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── create_review_request ────────────────────────────────────────────────

    #[test]
    fn test_create_review_request_empty_diff() {
        // Empty diff should return an error indicating no changes found
        let result = create_review_request("test-repo", "main", None, "", "");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("No file changes found"),
            "expected 'No file changes found', got: {}",
            err
        );
    }

    #[test]
    fn test_create_review_request_whitespace_only_diff() {
        // Whitespace-only diff is treated as empty after trimming
        let result = create_review_request("test-repo", "main", None, "   \n\t  ", "");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("No file changes found"),
            "expected 'No file changes found', got: {}",
            err
        );
    }

    #[test]
    fn test_create_review_request_valid_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index abc123..def456 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
     println!("hello");
+    println!("world");
 }"#;
        let result =
            create_review_request("my-repo", "feature-x", Some("main"), diff, "add greeting");
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.repo_name, "my-repo");
        assert_eq!(req.branch, "feature-x");
        assert_eq!(req.base_branch, Some("main".to_string()));
        assert_eq!(req.context, "add greeting");
        assert!(!req.files.is_empty());
    }

    #[test]
    fn test_create_review_request_no_base_branch() {
        let diff = r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 pub fn foo() {}
+pub fn bar() {}
"#;
        let result = create_review_request("repo", "HEAD", None, diff, "");
        assert!(result.is_ok());
        let req = result.unwrap();
        assert_eq!(req.base_branch, None);
        assert!(req.context.is_empty());
    }

    #[test]
    fn test_create_review_request_with_context() {
        let diff = r#"diff --git a/Cargo.toml b/Cargo.toml
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -1,3 +1,4 @@
 [package]
 name = "test"
+version = "0.1.0"
"#;
        let result = create_review_request("test", "main", None, diff, "Initial version bump");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().context, "Initial version bump");
    }

    // ─── print_diff_stats ─────────────────────────────────────────────────────

    #[test]
    fn test_print_diff_stats_single_file() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 line1
-line2
+line2 modified
+line3
 line4"#;
        let result = print_diff_stats(diff);
        assert!(result.is_ok());

        let stats = DiffStats::from_diff(diff).unwrap();
        assert_eq!(stats.files_changed, 1);
        assert_eq!(stats.files_added, 0);
        assert_eq!(stats.files_modified, 1);
        assert_eq!(stats.files_deleted, 0);
        assert_eq!(stats.insertions, 2);
        assert_eq!(stats.deletions, 1);
    }

    #[test]
    fn test_print_diff_stats_new_and_deleted_files() {
        let diff = r#"diff --git a/new_file.rs b/new_file.rs
new file mode 100644
--- /dev/null
+++ b/new_file.rs
@@ -0,0 +1,2 @@
+fn new() {}
diff --git a/old_file.rs b/old_file.rs
deleted file mode 100644
--- b/old_file.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-fn old() {}
"#;
        let stats = DiffStats::from_diff(diff).unwrap();
        assert_eq!(stats.files_changed, 2);
        assert_eq!(stats.files_added, 1);
        assert_eq!(stats.files_modified, 0);
        assert_eq!(stats.files_deleted, 1);
    }

    #[test]
    fn test_print_diff_stats_binary_file() {
        let diff = r#"diff --git a/logo.png b/logo.png
new file mode 100644
Binary files /dev/null and b/logo.png differ
"#;
        let stats = DiffStats::from_diff(diff).unwrap();
        assert_eq!(stats.files_changed, 1);
        assert_eq!(stats.files_added, 1);
        assert_eq!(stats.insertions, 0);
        assert_eq!(stats.deletions, 0);
    }

    #[test]
    fn test_print_diff_stats_empty_string() {
        let stats = DiffStats::from_diff("").unwrap();
        assert_eq!(stats.files_changed, 0);
        assert_eq!(stats.insertions, 0);
        assert_eq!(stats.deletions, 0);
    }

    // ─── print_complexity ─────────────────────────────────────────────────────

    #[test]
    fn test_print_complexity_multiple_files() {
        let changes = vec![
            FileChange {
                path: "src/main.rs".to_string(),
                change_type: gitforge_ai::ChangeType::Modified,
                diff: "+fn new_fn() {}".to_string(),
                language: Some("rust".to_string()),
            },
            FileChange {
                path: "src/lib.rs".to_string(),
                change_type: gitforge_ai::ChangeType::Added,
                diff: "+pub fn added() {}".to_string(),
                language: Some("rust".to_string()),
            },
        ];
        print_complexity(&changes); // just ensure it doesn't panic
    }

    #[test]
    fn test_print_complexity_detects_test_files() {
        let changes = vec![
            FileChange {
                path: "src/main.rs".to_string(),
                change_type: gitforge_ai::ChangeType::Modified,
                diff: "-old\n+new".to_string(),
                language: Some("rust".to_string()),
            },
            FileChange {
                path: "tests/integration_test.rs".to_string(),
                change_type: gitforge_ai::ChangeType::Added,
                diff: "+#[test]".to_string(),
                language: Some("rust".to_string()),
            },
        ];
        let complexity = ChangeComplexity::analyze(&changes);
        assert!(complexity.has_test_changes);
        assert!(!complexity.has_docs_changes);
        assert_eq!(complexity.files_touched, 2);
    }

    #[test]
    fn test_print_complexity_detects_docs() {
        let changes = vec![FileChange {
            path: "README.md".to_string(),
            change_type: gitforge_ai::ChangeType::Modified,
            diff: "+## New section".to_string(),
            language: Some("markdown".to_string()),
        }];
        let complexity = ChangeComplexity::analyze(&changes);
        assert!(complexity.has_docs_changes);
    }

    #[test]
    fn test_print_complexity_churn_calculation() {
        // A diff with 3 additions and 2 deletions = 5 churn
        let changes = vec![FileChange {
            path: "src/lib.rs".to_string(),
            change_type: gitforge_ai::ChangeType::Modified,
            diff: "+line1\n+line2\n+line3\n-old1\n-old2".to_string(),
            language: Some("rust".to_string()),
        }];
        let complexity = ChangeComplexity::analyze(&changes);
        assert_eq!(complexity.churn, 5);
    }

    #[test]
    fn test_print_complexity_empty() {
        let changes: Vec<FileChange> = vec![];
        let complexity = ChangeComplexity::analyze(&changes);
        assert_eq!(complexity.files_touched, 0);
        assert_eq!(complexity.total_lines, 0);
        assert_eq!(complexity.churn, 0);
        assert!(!complexity.has_test_changes);
        assert!(!complexity.has_docs_changes);
    }

    // ─── get_current_branch ──────────────────────────────────────────────────

    #[test]
    fn test_get_current_branch_fails_on_nonexistent_dir() {
        let result = get_current_branch(std::path::Path::new("/nonexistent/path/to/repo"));
        // A non-git directory should produce an error
        assert!(result.is_err());
    }

    // ─── provider_type_from_name ───────────────────────────────────────────────

    #[test]
    fn test_provider_type_from_name_anthropic() {
        let result = provider_type_from_name("anthropic");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProviderType::Anthropic);
    }

    #[test]
    fn test_provider_type_from_name_anthropic_case_insensitive() {
        assert!(provider_type_from_name("Anthropic").is_ok());
        assert!(provider_type_from_name("ANTHROPIC").is_ok());
        assert!(provider_type_from_name("AnThRoPiC").is_ok());
    }

    #[test]
    fn test_provider_type_from_name_openai() {
        let result = provider_type_from_name("openai");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProviderType::OpenAI);
    }

    #[test]
    fn test_provider_type_from_name_openai_case_insensitive() {
        assert!(provider_type_from_name("OpenAI").is_ok());
        assert!(provider_type_from_name("OPENAI").is_ok());
    }

    #[test]
    fn test_provider_type_from_name_ollama() {
        let result = provider_type_from_name("ollama");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProviderType::Ollama);
    }

    #[test]
    fn test_provider_type_from_name_ollama_case_insensitive() {
        assert!(provider_type_from_name("Ollama").is_ok());
        assert!(provider_type_from_name("OLLAMA").is_ok());
    }

    #[test]
    fn test_provider_type_from_name_unknown() {
        let result = provider_type_from_name("not_a_provider");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Unknown provider"),
            "expected 'Unknown provider', got: {}",
            err
        );
        assert!(
            err.to_string().contains("not_a_provider"),
            "expected provider name in error, got: {}",
            err
        );
    }

    #[test]
    fn test_provider_type_from_name_unknown_includes_suggestions() {
        let err = provider_type_from_name("openaii").unwrap_err().to_string();
        // Error should mention the unknown name
        assert!(err.contains("openaii"), "got: {}", err);
        // Error should list valid options
        assert!(err.contains("anthropic"), "got: {}", err);
        assert!(err.contains("openai"), "got: {}", err);
        assert!(err.contains("ollama"), "got: {}", err);
    }
}
