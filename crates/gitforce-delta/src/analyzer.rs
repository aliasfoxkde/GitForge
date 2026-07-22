//! Delta Analyzer — parses git diffs and builds execution plans

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Represents how a file changed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    /// New file added
    Added,
    /// Existing file modified
    Modified,
    /// File deleted
    Deleted,
    /// File renamed
    Renamed,
    /// File type changed (e.g., regular → symlink)
    TypeChanged,
}

impl From<&str> for ChangeType {
    fn from(s: &str) -> Self {
        match s {
            "A" => ChangeType::Added,
            "M" => ChangeType::Modified,
            "D" => ChangeType::Deleted,
            "R" => ChangeType::Renamed,
            "T" => ChangeType::TypeChanged,
            _ => ChangeType::Modified,
        }
    }
}

/// A single changed file
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub change_type: ChangeType,
    /// Number of lines added
    pub lines_added: usize,
    /// Number of lines deleted
    pub lines_deleted: usize,
}

/// Determines the execution scope based on change magnitude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionScope {
    /// Single file, trivial change → lint only
    Trivial,
    /// Few files, localized → fast lint + unit test
    Fast,
    /// Moderate changes → full lint + test + coverage
    Standard,
    /// Large changes → full pipeline + security + complexity gate
    Full,
    /// Architectural/core changes → everything including rebuilds
    Heavy,
}

impl ExecutionScope {
    /// Classify scope from file count and change magnitude
    pub fn from_change_count(count: usize, lines_changed: usize) -> Self {
        if count == 1 && lines_changed < 10 {
            ExecutionScope::Trivial
        } else if count <= 5 && lines_changed < 100 {
            ExecutionScope::Fast
        } else if count <= 20 {
            ExecutionScope::Standard
        } else if count <= 100 {
            ExecutionScope::Heavy
        } else {
            ExecutionScope::Full
        }
    }

    /// Whether to run security scans for this scope
    pub fn includes_security(&self) -> bool {
        matches!(self, ExecutionScope::Standard | ExecutionScope::Full | ExecutionScope::Heavy)
    }

    /// Whether to run E2E tests for this scope
    pub fn includes_e2e(&self) -> bool {
        matches!(self, ExecutionScope::Full | ExecutionScope::Heavy)
    }

    /// Whether to run coverage enforcement
    pub fn includes_coverage(&self) -> bool {
        !matches!(self, ExecutionScope::Trivial)
    }
}

/// The delta execution plan
#[derive(Debug, Clone)]
pub struct DeltaPlan {
    /// All changed files
    pub changed_files: Vec<ChangedFile>,
    /// Directories/packages directly affected
    pub affected_packages: Vec<String>,
    /// Packages affected through dependency chain
    pub transitive_packages: Vec<String>,
    /// Languages detected in changed files
    pub detected_languages: Vec<String>,
    /// Recommended execution scope
    pub execution_scope: ExecutionScope,
    /// Whether this is a docs-only change
    pub docs_only: bool,
    /// Whether this is a config-only change
    pub config_only: bool,
    /// Whether this changes CI/CD infrastructure
    pub ci_changed: bool,
    /// File count
    pub file_count: usize,
    /// Total lines changed
    pub total_lines_changed: usize,
}

impl DeltaPlan {
    /// Returns a human-readable summary of what to run
    pub fn execution_summary(&self) -> String {
        let scope = match self.execution_scope {
            ExecutionScope::Trivial => "lint only (trivial change)",
            ExecutionScope::Fast => "lint + unit tests (fast)",
            ExecutionScope::Standard => "lint + tests + coverage (standard)",
            ExecutionScope::Full => "full pipeline: lint + test + build + security (full)",
            ExecutionScope::Heavy => "heavy pipeline: all checks + complexity gate (heavy)",
        };

        let mut parts = vec![format!("scope: {}", scope)];

        if !self.detected_languages.is_empty() {
            parts.push(format!("languages: {}", self.detected_languages.join(", ")));
        }

        if !self.affected_packages.is_empty() {
            let pkgs: Vec<&str> = self.affected_packages.iter().map(|s| s.as_str()).take(5).collect();
            let suffix = if self.affected_packages.len() > 5 {
                format!(" + {} more", self.affected_packages.len() - 5)
            } else {
                String::new()
            };
            parts.push(format!("packages: {}{}", pkgs.join(", "), suffix));
        }

        if self.docs_only {
            parts.push("docs-only: true (skip build/test)".to_string());
        }

        if self.ci_changed {
            parts.push("ci-config changed: full validation recommended".to_string());
        }

        parts.join("\n  ")
    }
}

/// Analyzes git diffs to build delta execution plans
pub struct DeltaAnalyzer {
    repo_root: String,
    base_ref: String,
    head_ref: Option<String>,
}

impl DeltaAnalyzer {
    /// Create a new analyzer for the given repo
    ///
    /// `base_ref` — the commit/branch to diff against (e.g., "HEAD~1", "origin/main")
    /// `head_ref` — optional specific ref (defaults to current HEAD)
    pub fn new(repo_root: impl Into<String>, base_ref: impl Into<String>) -> Self {
        Self {
            repo_root: repo_root.into(),
            base_ref: base_ref.into(),
            head_ref: None,
        }
    }

    /// Set the head ref explicitly
    pub fn with_head_ref(mut self, head: impl Into<String>) -> Self {
        self.head_ref = Some(head.into());
        self
    }

    /// Run git diff and parse the output
    pub async fn analyze(&self) -> Result<DeltaPlan> {
        let diff_output = self.run_git_diff()?;
        let changed_files = self.parse_diff(&diff_output);
        let (affected_packages, transitive_packages) = self.map_packages(&changed_files);
        let detected_languages = self.detect_languages(&changed_files);
        let (docs_only, config_only, ci_changed) = self.categorize_changes(&changed_files);

        let file_count = changed_files.len();
        let total_lines_changed: usize = changed_files
            .iter()
            .map(|f| f.lines_added + f.lines_deleted)
            .sum();

        let scope = ExecutionScope::from_change_count(file_count, total_lines_changed);

        Ok(DeltaPlan {
            changed_files,
            affected_packages,
            transitive_packages,
            detected_languages,
            execution_scope: scope,
            docs_only,
            config_only,
            ci_changed,
            file_count,
            total_lines_changed,
        })
    }

    /// Run git diff --stat and --numstat
    fn run_git_diff(&self) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.args([
            "diff",
            "--numstat",
            "--raw",
            "--no-color",
            &self.base_ref,
        ]);
        if let Some(ref head) = self.head_ref {
            cmd.arg(head);
        }
        cmd.current_dir(&self.repo_root);

        let output = cmd
            .output()
            .context("failed to run git diff")?;

        if !output.status.success() {
            anyhow::bail!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Parse git diff output into structured ChangedFile entries
    fn parse_diff(&self, output: &str) -> Vec<ChangedFile> {
        output
            .lines()
            .filter_map(|line| {
                // numstat format: <lines_added>\t<lines_deleted>\t<path>
                // raw format: :<old_mode> <new_mode> <old_sha> <new_sha> <status>\t<new_path>
                let parts: Vec<&str> = line.splitn(4, '\t').collect();
                if parts.len() < 2 {
                    return None;
                }

                // Try numstat format first
                if parts[0].chars().all(|c| c.is_ascii_digit() || c == '-') {
                    let lines_added = parts[0].parse::<usize>().unwrap_or(0);
                    let lines_deleted = parts[1].parse::<usize>().unwrap_or(0);
                    let path = parts[2].trim();

                    if path == "/dev/null" || path.is_empty() {
                        return None;
                    }

                    // Detect change type from status char in raw output
                    let change_type = if line.starts_with(":000000") {
                        ChangeType::Added
                    } else {
                        ChangeType::Modified
                    };

                    return Some(ChangedFile {
                        path: path.to_string(),
                        change_type,
                        lines_added,
                        lines_deleted,
                    });
                }

                // Try raw format
                if let Some(raw_part) = line.split('\t').last() {
                    let path = raw_part.trim().to_string();
                    if path.is_empty() || path == "/dev/null" {
                        return None;
                    }
                    return Some(ChangedFile {
                        path,
                        change_type: ChangeType::Modified,
                        lines_added: 0,
                        lines_deleted: 0,
                    });
                }

                None
            })
            .collect()
    }

    /// Map changed files to their owning packages
    fn map_packages(&self, files: &[ChangedFile]) -> (Vec<String>, Vec<String>) {
        let mut packages: HashSet<String> = HashSet::new();
        let transitive: HashSet<String> = HashSet::new();

        for file in files {
            let path = Path::new(&file.path);

            // Go packages
            if path.ends_with("go.mod") || path.ends_with("go.sum") {
                packages.insert("go".to_string());
            } else if file.path.contains("/go/") || file.path.ends_with(".go") {
                if let Some(parent) = path.parent() {
                    // Find the closest go.mod ancestor
                    let go_mod_dir = self.find_go_module(parent);
                    if let Some(mod_dir) = go_mod_dir {
                        packages.insert(format!("go:{}", mod_dir));
                    } else {
                        packages.insert("go:.".to_string());
                    }
                }
            }

            // Python packages
            else if file.path.contains("/pyproject.toml")
                || file.path.contains("/setup.py")
                || file.path.ends_with(".py")
            {
                packages.insert("python".to_string());
            }

            // Rust crates
            else if file.path.contains("/Cargo.toml")
                || file.path.contains("/Cargo.lock")
                || file.path.ends_with(".rs")
            {
                if let Some(parent) = path.parent() {
                    let cargo_toml = self.find_cargo_toml(parent);
                    if let Some(ct) = cargo_toml {
                        packages.insert(format!("rust:{}", ct));
                    } else {
                        packages.insert("rust:.".to_string());
                    }
                }
            }

            // TypeScript / Node
            else if file.path.contains("/package.json")
                || file.path.contains("/tsconfig.json")
                || file.path.ends_with(".ts")
                || file.path.ends_with(".tsx")
                || file.path.ends_with(".js")
            {
                packages.insert("typescript".to_string());
            }

            // Shell scripts
            else if file.path.ends_with(".sh") {
                packages.insert("shell".to_string());
            }

            // Docs
            else if file.path.starts_with("docs/")
                || file.path.ends_with(".md")
                || file.path.ends_with(".rst")
            {
                packages.insert("docs".to_string());
            }

            // CI/CD config
            else if file.path.contains(".github/workflows/")
                || file.path.contains(".githooks/")
                || file.path == ".github"
            {
                packages.insert("ci-cd".to_string());
            }
        }

        // Build transitive dependency closure
        // (simplified — in production would consult a dependency graph DB)
        let transitive: Vec<String> = transitive.into_iter().collect();

        let mut affected: Vec<String> = packages.into_iter().collect();
        affected.sort();

        (affected, transitive)
    }

    /// Find the closest go.mod ancestor directory
    fn find_go_module(&self, path: &Path) -> Option<String> {
        let mut current = PathBuf::from(path);
        loop {
            if current.join("go.mod").exists() {
                return current.to_str().map(String::from);
            }
            if !current.pop() {
                break;
            }
        }
        None
    }

    /// Find the closest Cargo.toml ancestor directory
    fn find_cargo_toml(&self, path: &Path) -> Option<String> {
        let mut current = PathBuf::from(path);
        loop {
            if current.join("Cargo.toml").exists() {
                return current.to_str().map(String::from);
            }
            if !current.pop() {
                break;
            }
        }
        None
    }

    /// Detect languages from changed files
    fn detect_languages(&self, files: &[ChangedFile]) -> Vec<String> {
        let mut langs: HashSet<&str> = HashSet::new();

        for file in files {
            if file.path.ends_with(".go") {
                langs.insert("go");
            } else if file.path.ends_with(".py") {
                langs.insert("python");
            } else if file.path.ends_with(".rs") {
                langs.insert("rust");
            } else if file.path.ends_with(".ts") || file.path.ends_with(".tsx") {
                langs.insert("typescript");
            } else if file.path.ends_with(".js") || file.path.ends_with(".jsx") {
                langs.insert("javascript");
            } else if file.path.ends_with(".sh") {
                langs.insert("shell");
            } else if file.path.ends_with(".yaml") || file.path.ends_with(".yml") {
                langs.insert("yaml");
            }
        }

        let mut result: Vec<String> = langs.into_iter().map(String::from).collect();
        result.sort();
        result
    }

    /// Categorize the change set
    fn categorize_changes(&self, files: &[ChangedFile]) -> (bool, bool, bool) {
        let docs_extensions = [".md", ".rst", ".txt", ".pdf", ".doc"];
        let config_paths = [
            ".github/workflows/",
            ".githooks/",
            ".github/actions/",
            "pyproject.toml",
            "go.mod",
            "Cargo.toml",
            "package.json",
            "Makefile",
            ".golangci.yml",
            ".ruff.toml",
            "tsconfig.json",
        ];

        let docs_only = files.iter().all(|f| {
            docs_extensions.iter().any(|ext| f.path.ends_with(ext))
                || f.path.starts_with("docs/")
        });

        let config_only = files.iter().all(|f| {
            config_paths.iter().any(|cp| f.path.contains(cp))
                || f.path.ends_with(".toml")
                || f.path.ends_with(".yaml")
                || f.path.ends_with(".yml")
                || f.path.ends_with(".json")
        });

        let ci_changed = files
            .iter()
            .any(|f| f.path.contains(".github/workflows/") || f.path.contains(".githooks/"));

        (docs_only, config_only, ci_changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_scope_classification() {
        assert_eq!(ExecutionScope::from_change_count(1, 5), ExecutionScope::Trivial);
        assert_eq!(ExecutionScope::from_change_count(3, 80), ExecutionScope::Fast);
        assert_eq!(ExecutionScope::from_change_count(15, 500), ExecutionScope::Standard);
        assert_eq!(ExecutionScope::from_change_count(50, 2000), ExecutionScope::Heavy);
        assert_eq!(ExecutionScope::from_change_count(200, 5000), ExecutionScope::Full);
    }

    #[test]
    fn test_scope_security() {
        assert!(!ExecutionScope::Trivial.includes_security());
        assert!(ExecutionScope::Standard.includes_security());
        assert!(ExecutionScope::Full.includes_security());
    }
}
