//! Fix Suggestion Engine
//!
//! Generates code fixes based on review findings and can apply them.

use crate::{HunkLine, ParsedDiff};
use gitforce_ai::{FindingCategory, ReviewFinding, Severity};
use regex::Regex;
use std::collections::HashMap;

/// A suggested code fix
#[derive(Debug, Clone)]
pub struct FixSuggestion {
    /// File to fix
    pub file: String,
    /// Line number to fix
    pub line: u32,
    /// Original code
    pub original: String,
    /// Fixed code
    pub fixed: String,
    /// Explanation of the fix
    pub explanation: String,
    /// Category of the fix
    pub category: FixCategory,
}

/// Categories of fixes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixCategory {
    SecurityHardening,
    ParameterizedQuery,
    InputValidation,
    UseEnvironmentVariable,
    SecureRandom,
    UseSafeApi,
    AddAuth,
    AddRateLimit,
    UseConstants,
    SafeDeserialization,
    PathNormalization,
    EscapeOutput,
    Other,
}

impl FixCategory {
    pub fn from_finding_category(cat: FindingCategory) -> Self {
        match cat {
            FindingCategory::Security => FixCategory::SecurityHardening,
            FindingCategory::Bug => FixCategory::UseSafeApi,
            FindingCategory::BestPractice => FixCategory::Other,
            _ => FixCategory::Other,
        }
    }
}

/// Generator for code fixes based on findings
pub struct FixGenerator {
    /// Fix templates by vulnerability type
    templates: HashMap<String, FixTemplate>,
}

struct FixTemplate {
    pattern: Regex,
    generate: fn(&str) -> Option<FixSuggestion>,
}

impl Default for FixGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl FixGenerator {
    /// Create a new fix generator with default templates
    pub fn new() -> Self {
        let mut templates = HashMap::new();

        // SQL Injection fixes
        templates.insert(
            "sql_injection_concat".to_string(),
            FixTemplate {
                pattern: Regex::new(r#"(\w+)\s*\(\s*["'][^"']*\+[^"']*["']\s*\)"#).unwrap(),
                generate: |code: &str| {
                    // Convert string concatenation to parameterized query
                    Some(FixSuggestion {
                        file: String::new(),
                        line: 1,
                        original: code.to_string(),
                        fixed: code
                            .replace("+", "?")
                            .replace("'\"+", "\"")
                            .replace("+'\"", "\""),
                        explanation: "Use parameterized queries instead of string concatenation".to_string(),
                        category: FixCategory::ParameterizedQuery,
                    })
                },
            },
        );

        // Hardcoded password fixes
        templates.insert(
            "hardcoded_password".to_string(),
            FixTemplate {
                pattern: Regex::new(r#"(\w+)\s*=\s*["'][^"']{4,}["']"#).unwrap(),
                generate: |code: &str| {
                    let var_name = code.split('=').next()?.trim();
                    Some(FixSuggestion {
                        file: String::new(),
                        line: 1,
                        original: code.to_string(),
                        fixed: format!("{} = std::env::var(\"{}\").expect(\"{} must be set\")", var_name, var_name.to_uppercase(), var_name),
                        explanation: "Use environment variables for sensitive data".to_string(),
                        category: FixCategory::UseEnvironmentVariable,
                    })
                },
            },
        );

        // Weak hash fixes
        templates.insert(
            "weak_hash".to_string(),
            FixTemplate {
                pattern: Regex::new(r#"(md5|sha1|des|rc4)\s*\("#).unwrap(),
                generate: |code: &str| {
                    Some(FixSuggestion {
                        file: String::new(),
                        line: 1,
                        original: code.to_string(),
                        fixed: code.replace("$1(", "sha256("),
                        explanation: "Use SHA-256 or a cryptographically secure hash function".to_string(),
                        category: FixCategory::SecurityHardening,
                    })
                },
            },
        );

        // Insecure random fixes
        templates.insert(
            "insecure_random".to_string(),
            FixTemplate {
                pattern: Regex::new(r#"Math\.random\s*\("#).unwrap(),
                generate: |_code: &str| {
                    Some(FixSuggestion {
                        file: String::new(),
                        line: 1,
                        original: "Math.random()".to_string(),
                        fixed: "crypto.getRandomValues(new Uint32Array(1))[0]".to_string(),
                        explanation: "Use crypto.getRandomValues() for cryptographically secure random numbers".to_string(),
                        category: FixCategory::SecureRandom,
                    })
                },
            },
        );

        // eval() fixes
        templates.insert(
            "eval_usage".to_string(),
            FixTemplate {
                pattern: Regex::new(r#"eval\s*\("#).unwrap(),
                generate: |_code: &str| {
                    Some(FixSuggestion {
                        file: String::new(),
                        line: 1,
                        original: "eval(...)".to_string(),
                        fixed: "// Consider using JSON.parse() or a safer alternative".to_string(),
                        explanation: "Avoid eval() - it can execute arbitrary code".to_string(),
                        category: FixCategory::SecurityHardening,
                    })
                },
            },
        );

        Self { templates }
    }

    /// Generate a fix for a given finding
    pub fn generate_fix(&self, finding: &ReviewFinding) -> Option<FixSuggestion> {
        // Try each template
        for template in self.templates.values() {
            if template.pattern.is_match(&finding.description) {
                if let Some(fix) = (template.generate)(&finding.description) {
                    return Some(FixSuggestion {
                        file: finding.file.clone(),
                        line: finding.line_start.unwrap_or(1),
                        ..fix
                    });
                }
            }
        }

        // If AI suggestion exists, use that
        if let Some(ref suggestion) = finding.suggestion {
            return Some(FixSuggestion {
                file: finding.file.clone(),
                line: finding.line_start.unwrap_or(1),
                original: finding.code_snippet.clone().unwrap_or_default(),
                fixed: suggestion.clone(),
                explanation: "AI suggested fix".to_string(),
                category: FixCategory::from_finding_category(finding.category),
            });
        }

        None
    }

    /// Generate fixes for multiple findings
    pub fn generate_fixes(&self, findings: &[ReviewFinding]) -> Vec<FixSuggestion> {
        findings
            .iter()
            .filter_map(|f| self.generate_fix(f))
            .collect()
    }
}

/// Apply fixes to a parsed diff
pub fn apply_fixes_to_diff(diff: &ParsedDiff, fixes: &[FixSuggestion]) -> ParsedDiff {
    let mut new_diff = diff.clone();

    for fix in fixes {
        // Find and replace in hunks
        for hunk in &mut new_diff.hunks {
            for line in &mut hunk.lines {
                match line {
                    HunkLine::Addition(s) if s.contains(&fix.original) => {
                        *s = s.replace(&fix.original, &fix.fixed);
                    }
                    HunkLine::Context(s) if s.contains(&fix.original) => {
                        *s = s.replace(&fix.original, &fix.fixed);
                    }
                    _ => {}
                }
            }
        }
    }

    new_diff
}

/// Create a patch from fixes
pub fn create_patch(file: &str, fixes: &[FixSuggestion]) -> String {
    let mut patch = String::new();

    for fix in fixes {
        patch.push_str(&format!(
            "--- a/{}\n+++ b/{}\n@@ -{},0 +{},0 @@\n-{}\n+{}\n",
            file, file, fix.line, fix.line, fix.original, fix.fixed
        ));
    }

    patch
}

/// Check if a finding can be auto-fixed
pub fn is_auto_fixable(finding: &ReviewFinding) -> bool {
    // Only certain severity levels are auto-fixable
    matches!(
        finding.severity,
        Severity::Low | Severity::Medium | Severity::High
    ) && !matches!(
        finding.category,
        FindingCategory::Security
    ) // Security issues need human review
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_fix_for_password() {
        let generator = FixGenerator::new();
        let finding = ReviewFinding {
            file: "config.py".to_string(),
            line_start: Some(10),
            line_end: Some(10),
            severity: Severity::High,
            category: FindingCategory::Security,
            title: "Hardcoded password".to_string(),
            description: "password = 'supersecret'".to_string(),
            suggestion: None,
            code_snippet: None,
        };

        let fix = generator.generate_fix(&finding);
        assert!(fix.is_some());
    }

    #[test]
    fn test_is_auto_fixable() {
        let low_finding = ReviewFinding {
            file: "test.rs".to_string(),
            line_start: Some(1),
            line_end: Some(1),
            severity: Severity::Low,
            category: FindingCategory::Style,
            title: "Style issue".to_string(),
            description: "Style".to_string(),
            suggestion: None,
            code_snippet: None,
        };

        let security_finding = ReviewFinding {
            file: "test.rs".to_string(),
            line_start: Some(1),
            line_end: Some(1),
            severity: Severity::High,
            category: FindingCategory::Security,
            title: "Security issue".to_string(),
            description: "Security".to_string(),
            suggestion: None,
            code_snippet: None,
        };

        assert!(is_auto_fixable(&low_finding));
        assert!(!is_auto_fixable(&security_finding));
    }
}
