//! GitForce Code Review Module
//!
//! This crate provides diff parsing and analysis for AI-powered code review.

pub mod domain;
pub mod security;

use gitforge_ai::{ChangeType, FileChange};
use regex::Regex;
use thiserror::Error;

/// Errors for review operations
#[derive(Debug, Error)]
pub enum ReviewError {
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A parsed hunk from a diff
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// Starting line number in original file
    pub old_start: u32,
    /// Number of lines in original file
    pub old_lines: u32,
    /// Starting line number in new file
    pub new_start: u32,
    /// Number of lines in new file
    pub new_lines: u32,
    /// Lines in this hunk
    pub lines: Vec<HunkLine>,
}

/// A single line in a hunk
#[derive(Debug, Clone)]
pub enum HunkLine {
    /// Context line (unchanged)
    Context(String),
    /// Addition (new line)
    Addition(String),
    /// Deletion (removed line)
    Deletion(String),
}

/// A parsed file diff
#[derive(Debug, Clone)]
pub struct ParsedDiff {
    /// Old file path (None for new files)
    pub old_path: Option<String>,
    /// New file path (None for deleted files)
    pub new_path: Option<String>,
    /// Whether this is a new file
    pub is_new: bool,
    /// Whether this is a deleted file
    pub is_deleted: bool,
    /// Whether this is a binary file
    pub is_binary: bool,
    /// hunks in this diff
    pub hunks: Vec<DiffHunk>,
    /// Language detected for this file
    pub language: Option<String>,
}

impl ParsedDiff {
    /// Check if this diff has any additions
    pub fn has_additions(&self) -> bool {
        self.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| matches!(l, HunkLine::Addition(_))))
    }

    /// Check if this diff has any deletions
    pub fn has_deletions(&self) -> bool {
        self.hunks
            .iter()
            .any(|h| h.lines.iter().any(|l| matches!(l, HunkLine::Deletion(_))))
    }
}

/// Language detection based on file extension
fn detect_language(path: &str) -> Option<String> {
    let ext = path.rsplit('.').next()?;
    match ext.to_lowercase().as_str() {
        "rs" => Some("rust".to_string()),
        "js" => Some("javascript".to_string()),
        "ts" => Some("typescript".to_string()),
        "jsx" => Some("javascript".to_string()),
        "tsx" => Some("typescript".to_string()),
        "py" => Some("python".to_string()),
        "go" => Some("go".to_string()),
        "java" => Some("java".to_string()),
        "c" => Some("c".to_string()),
        "cpp" | "cc" | "cxx" => Some("cpp".to_string()),
        "h" | "hpp" => Some("header".to_string()),
        "cs" => Some("csharp".to_string()),
        "rb" => Some("ruby".to_string()),
        "php" => Some("php".to_string()),
        "swift" => Some("swift".to_string()),
        "kt" | "kts" => Some("kotlin".to_string()),
        "scala" => Some("scala".to_string()),
        "md" | "markdown" => Some("markdown".to_string()),
        "json" => Some("json".to_string()),
        "yaml" | "yml" => Some("yaml".to_string()),
        "toml" => Some("toml".to_string()),
        "xml" => Some("xml".to_string()),
        "html" | "htm" => Some("html".to_string()),
        "css" => Some("css".to_string()),
        "scss" | "sass" => Some("scss".to_string()),
        "sql" => Some("sql".to_string()),
        "sh" | "bash" => Some("shell".to_string()),
        "zsh" => Some("zsh".to_string()),
        "fish" => Some("fish".to_string()),
        "ps1" => Some("powershell".to_string()),
        "dockerfile" => Some("dockerfile".to_string()),
        "tf" => Some("terraform".to_string()),
        "proto" => Some("protobuf".to_string()),
        _ => None,
    }
}

/// Parse a unified diff format string
pub fn parse_unified_diff(diff: &str) -> Result<Vec<ParsedDiff>, ReviewError> {
    let mut diffs = Vec::new();
    let mut current_diff: Option<ParsedDiff> = None;
    let mut current_hunk: Option<DiffHunk> = None;

    // Regex patterns for unified diff format
    let diff_header_re = Regex::new(r"^diff --git a/(.*) b/(.*)$").unwrap();
    let new_file_re = Regex::new(r"^new file mode \d+$").unwrap();
    let deleted_file_re = Regex::new(r"^deleted file mode \d+$").unwrap();
    let index_re = Regex::new(r"^index [a-f0-9]+\.\.[a-f0-9]+").unwrap();
    let hunk_header_re = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();
    let binary_re = Regex::new(r"^Binary files").unwrap();

    for line in diff.lines() {
        // Check for diff header
        if let Some(caps) = diff_header_re.captures(line) {
            // Save previous diff if exists
            if let Some(mut d) = current_diff.take() {
                if let Some(h) = current_hunk.take() {
                    d.hunks.push(h);
                }
                diffs.push(d);
            }

            let old_path = caps.get(1).map(|m| m.as_str().to_string());
            let new_path = caps.get(2).map(|m| m.as_str().to_string());

            current_diff = Some(ParsedDiff {
                old_path,
                new_path: new_path.clone(),
                is_new: false,
                is_deleted: false,
                is_binary: false,
                hunks: Vec::new(),
                language: new_path.as_ref().and_then(|p| detect_language(p)),
            });
            continue;
        }

        // Check for new/deleted file markers
        if let Some(ref mut d) = current_diff {
            if new_file_re.is_match(line) {
                d.is_new = true;
            } else if deleted_file_re.is_match(line) {
                d.is_deleted = true;
            } else if index_re.is_match(line) {
                // Index line - skip
            } else if binary_re.is_match(line) {
                d.is_binary = true;
            } else if let Some(caps) = hunk_header_re.captures(line) {
                // Save previous hunk if exists
                if let Some(h) = current_hunk.take() {
                    d.hunks.push(h);
                }

                let old_start: u32 = caps
                    .get(1)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);
                let old_lines: u32 = caps
                    .get(2)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);
                let new_start: u32 = caps
                    .get(3)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);
                let new_lines: u32 = caps
                    .get(4)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);

                current_hunk = Some(DiffHunk {
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    lines: Vec::new(),
                });
            } else if current_hunk.is_some() {
                // Add line to current hunk
                if let Some(ref mut h) = current_hunk {
                    if let Some(c) = line.strip_prefix(' ') {
                        h.lines.push(HunkLine::Context(c.to_string()));
                    } else if let Some(c) = line.strip_prefix('+') {
                        h.lines.push(HunkLine::Addition(c.to_string()));
                    } else if let Some(c) = line.strip_prefix('-') {
                        h.lines.push(HunkLine::Deletion(c.to_string()));
                    }
                }
            }
        }
    }

    // Don't forget the last diff/hunk
    if let Some(mut d) = current_diff {
        if let Some(h) = current_hunk.take() {
            d.hunks.push(h);
        }
        diffs.push(d);
    }

    Ok(diffs)
}

/// Convert a parsed diff to FileChange for AI review
impl From<ParsedDiff> for FileChange {
    fn from(diff: ParsedDiff) -> Self {
        let path = diff.new_path.or(diff.old_path).unwrap_or_default();
        let change_type = if diff.is_new {
            ChangeType::Added
        } else if diff.is_deleted {
            ChangeType::Deleted
        } else {
            ChangeType::Modified
        };

        // Convert hunks to a simple diff string representation
        let diff_text = hunks_to_diff_string(&diff.hunks);

        FileChange {
            path,
            change_type,
            diff: diff_text,
            language: diff.language,
        }
    }
}

/// Convert hunks to a simple string representation
fn hunks_to_diff_string(hunks: &[DiffHunk]) -> String {
    let mut output = String::new();

    for hunk in hunks {
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        ));

        for line in &hunk.lines {
            match line {
                HunkLine::Context(c) => output.push_str(&format!(" {}\n", c)),
                HunkLine::Addition(a) => output.push_str(&format!("+{}\n", a)),
                HunkLine::Deletion(d) => output.push_str(&format!("-{}\n", d)),
            }
        }
    }

    output
}

/// Extract file changes from a git diff
pub fn extract_changes_from_diff(diff: &str) -> Result<Vec<FileChange>, ReviewError> {
    let parsed = parse_unified_diff(diff)?;
    Ok(parsed.into_iter().map(FileChange::from).collect())
}

/// Get statistics about a diff
#[derive(Debug)]
pub struct DiffStats {
    pub files_changed: usize,
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    pub insertions: usize,
    pub deletions: usize,
}

impl DiffStats {
    /// Calculate stats from a unified diff
    pub fn from_diff(diff: &str) -> Result<Self, ReviewError> {
        let parsed = parse_unified_diff(diff)?;

        let mut stats = DiffStats {
            files_changed: parsed.len(),
            files_added: 0,
            files_modified: 0,
            files_deleted: 0,
            insertions: 0,
            deletions: 0,
        };

        for p in &parsed {
            if p.is_new {
                stats.files_added += 1;
            } else if p.is_deleted {
                stats.files_deleted += 1;
            } else {
                stats.files_modified += 1;
            }

            for hunk in &p.hunks {
                for line in &hunk.lines {
                    match line {
                        HunkLine::Addition(_) => stats.insertions += 1,
                        HunkLine::Deletion(_) => stats.deletions += 1,
                        HunkLine::Context(_) => {}
                    }
                }
            }
        }

        Ok(stats)
    }
}

/// Analyze complexity of changes
#[derive(Debug)]
pub struct ChangeComplexity {
    pub total_lines: usize,
    pub churn: usize, // Total changes (adds + deletes)
    pub files_touched: usize,
    pub has_test_changes: bool,
    pub has_docs_changes: bool,
}

impl ChangeComplexity {
    /// Analyze complexity from a set of file changes
    pub fn analyze(changes: &[FileChange]) -> Self {
        let mut complexity = ChangeComplexity {
            total_lines: 0,
            churn: 0,
            files_touched: changes.len(),
            has_test_changes: false,
            has_docs_changes: false,
        };

        for change in changes {
            let lines = change.diff.lines().count();
            complexity.total_lines += lines;

            // Count additions and deletions as churn
            for line in change.diff.lines() {
                let is_addition = line.starts_with('+') && !line.starts_with("++");
                let is_deletion = line.starts_with('-') && !line.starts_with("--");
                if is_addition || is_deletion {
                    complexity.churn += 1;
                }
            }

            // Check for test files
            let path_lower = change.path.to_lowercase();
            if path_lower.contains("test")
                || path_lower.contains("_test")
                || path_lower.ends_with("_tests.rs")
            {
                complexity.has_test_changes = true;
            }

            // Check for docs
            if path_lower.contains("doc") || path_lower.ends_with(".md") {
                complexity.has_docs_changes = true;
            }
        }

        complexity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index abc123..def456 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,7 @@
 fn main() {
-    println!("old");
+    println!("new");
+    another_function();
 }
+
 fn another_function() {}"#;

        let parsed = parse_unified_diff(diff).unwrap();
        assert_eq!(parsed.len(), 1);

        let d = &parsed[0];
        assert!(!d.is_new);
        assert!(!d.is_deleted);
        assert_eq!(d.hunks.len(), 1);
    }

    #[test]
    fn test_diff_stats() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 line1
-line2
+line2 modified
+line3
 line4"#;

        let stats = DiffStats::from_diff(diff).unwrap();
        assert_eq!(stats.files_changed, 1);
        assert_eq!(stats.insertions, 2);
        assert_eq!(stats.deletions, 1);
    }

    #[test]
    fn test_change_complexity() {
        let changes = vec![
            FileChange {
                path: "src/main.rs".to_string(),
                change_type: ChangeType::Modified,
                diff: "+new line\n-old line".to_string(),
                language: Some("rust".to_string()),
            },
            FileChange {
                path: "tests/main_test.rs".to_string(),
                change_type: ChangeType::Added,
                diff: "+test case".to_string(),
                language: Some("rust".to_string()),
            },
        ];

        let complexity = ChangeComplexity::analyze(&changes);
        assert_eq!(complexity.files_touched, 2);
        assert!(complexity.has_test_changes);
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("main.rs"), Some("rust".to_string()));
        assert_eq!(detect_language("main.py"), Some("python".to_string()));
        assert_eq!(detect_language("main.go"), Some("go".to_string()));
        assert_eq!(detect_language("README.md"), Some("markdown".to_string()));
        assert_eq!(
            detect_language("Dockerfile"),
            Some("dockerfile".to_string())
        );
        assert_eq!(detect_language("unknown.xyz"), None);
    }
}
