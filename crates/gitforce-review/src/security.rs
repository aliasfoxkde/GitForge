//! Security Vulnerability Scanner
//!
//! Detects common security vulnerabilities in code changes.

use crate::{HunkLine, ParsedDiff};
use regex::Regex;

/// A detected security vulnerability
#[derive(Debug, Clone)]
pub struct Vulnerability {
    /// File path
    pub file: String,
    /// Starting line number
    pub line: u32,
    /// Vulnerability type
    pub vuln_type: VulnerabilityType,
    /// Description
    pub description: String,
    /// Suggested fix
    pub suggestion: Option<String>,
}

/// Types of vulnerabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VulnerabilityType {
    SqlInjection,
    CommandInjection,
    PathTraversal,
    Xss,
    SecretHardcoded,
    InsecureCrypto,
    UnsafeDeserialization,
    WeakHash,
    HardcodedCredential,
    InsecureRandom,
    UnvalidatedRedirect,
    MissingAuth,
    MissingRateLimit,
    BufferOverflow,
    MemoryLeak,
    RaceCondition,
    Other,
}

impl VulnerabilityType {
    pub fn severity(&self) -> &'static str {
        match self {
            VulnerabilityType::SqlInjection => "critical",
            VulnerabilityType::CommandInjection => "critical",
            VulnerabilityType::HardcodedCredential => "high",
            VulnerabilityType::SecretHardcoded => "high",
            VulnerabilityType::UnsafeDeserialization => "critical",
            VulnerabilityType::Xss => "high",
            VulnerabilityType::PathTraversal => "high",
            VulnerabilityType::InsecureCrypto => "high",
            VulnerabilityType::WeakHash => "medium",
            VulnerabilityType::InsecureRandom => "medium",
            VulnerabilityType::UnvalidatedRedirect => "medium",
            VulnerabilityType::MissingAuth => "high",
            VulnerabilityType::MissingRateLimit => "medium",
            VulnerabilityType::BufferOverflow => "critical",
            VulnerabilityType::MemoryLeak => "medium",
            VulnerabilityType::RaceCondition => "high",
            VulnerabilityType::Other => "low",
        }
    }
}

/// Security scanner for code changes
pub struct SecurityScanner {
    /// Patterns to detect vulnerabilities
    patterns: Vec<VulnerabilityPattern>,
}

struct VulnerabilityPattern {
    pattern: Regex,
    vuln_type: VulnerabilityType,
    description: &'static str,
    suggestion: &'static str,
    // File extensions this applies to (None = all)
    extensions: Option<Vec<&'static str>>,
}

impl Default for SecurityScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl SecurityScanner {
    /// Create a new security scanner with default patterns
    pub fn new() -> Self {
        let patterns = vec![
            // SQL Injection patterns
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:execute|exec|query|cursor)\s*\([^)]*\+"#).unwrap(),
                vuln_type: VulnerabilityType::SqlInjection,
                description: "Potential SQL injection - string concatenation in query",
                suggestion: "Use parameterized queries or an ORM",
                extensions: Some(vec!["py", "js", "ts", "java", "rb", "go", "rs", "php", "cs"]),
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)f["'].*?\{.*?\}.*?["']"#).unwrap(),
                vuln_type: VulnerabilityType::SqlInjection,
                description: "F-string/formatted string in SQL query",
                suggestion: "Use parameterized queries instead",
                extensions: Some(vec!["py", "js", "ts"]),
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)\.format\s*\(\s*["'].*?\%.*?["']"#).unwrap(),
                vuln_type: VulnerabilityType::SqlInjection,
                description: "String formatting in SQL query",
                suggestion: "Use parameterized queries",
                extensions: Some(vec!["py", "java", "rb"]),
            },
            // Command Injection patterns
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:system|exec|spawn|popen|shell_exec|exec\s*\()\s*\(\s*.*?(?:input|args?|cmd|command)"#).unwrap(),
                vuln_type: VulnerabilityType::CommandInjection,
                description: "Dynamic command execution with user input",
                suggestion: "Avoid shell commands, use proper input validation",
                extensions: None,
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:eval|Function\))\s*\("#).unwrap(),
                vuln_type: VulnerabilityType::CommandInjection,
                description: "Use of eval() or dynamic function execution",
                suggestion: "Avoid eval() - use safer alternatives",
                extensions: Some(vec!["js", "ts", "py", "rb", "php"]),
            },
            // Hardcoded secrets
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:api[_-]?key|secret[_-]??key|access[_-]?token|auth[_-]?token)\s*=\s*["'][a-zA-Z0-9_\-]{20,}["']"#).unwrap(),
                vuln_type: VulnerabilityType::HardcodedCredential,
                description: "Hardcoded API key or token detected",
                suggestion: "Use environment variables or a secrets manager",
                extensions: None,
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:password|passwd|pwd)\s*=\s*["'][^"']{4,}["']"#).unwrap(),
                vuln_type: VulnerabilityType::HardcodedCredential,
                description: "Hardcoded password detected",
                suggestion: "Use environment variables or secure password storage",
                extensions: None,
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----"#).unwrap(),
                vuln_type: VulnerabilityType::SecretHardcoded,
                description: "Hardcoded private key detected",
                suggestion: "Use a secrets manager, never commit private keys",
                extensions: None,
            },
            // Insecure cryptography
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:md5|sha1|des|rc4)\s*\("#).unwrap(),
                vuln_type: VulnerabilityType::WeakHash,
                description: "Use of weak hash algorithm (MD5, SHA1, DES, RC4)",
                suggestion: "Use SHA-256 or stronger, or dedicated crypto libraries",
                extensions: None,
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)Crypto\.createCipher\s*\("#).unwrap(),
                vuln_type: VulnerabilityType::InsecureCrypto,
                description: "Node.js Crypto createCipher is deprecated and insecure",
                suggestion: "Use crypto.createCipheriv with a proper IV",
                extensions: Some(vec!["js", "ts"]),
            },
            // Path traversal
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:readFile|readFileSync|open|include|require)\s*\(\s*.*?(?:fileName|file|path|filename)"#).unwrap(),
                vuln_type: VulnerabilityType::PathTraversal,
                description: "File operation with potentially unvalidated path",
                suggestion: "Validate and sanitize all file paths, use allowlists",
                extensions: Some(vec!["js", "ts", "py", "java", "rb", "go", "rs"]),
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"\.\./"#).unwrap(),
                vuln_type: VulnerabilityType::PathTraversal,
                description: "Path traversal sequence '../' detected",
                suggestion: "Ensure paths are validated and normalized",
                extensions: None,
            },
            // XSS patterns (for web code)
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:innerHTML|dangerouslySetInnerHTML|html\s*\(|document\.write)"#).unwrap(),
                vuln_type: VulnerabilityType::Xss,
                description: "Potential XSS - directly setting HTML content",
                suggestion: "Use textContent or sanitization libraries",
                extensions: Some(vec!["js", "ts"]),
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)echo\s+\$_GET|echo\s+\$_POST|print\s+\$_GET|print\s+\$_POST"#).unwrap(),
                vuln_type: VulnerabilityType::Xss,
                description: "Direct output of request parameters",
                suggestion: "Always sanitize and escape user input",
                extensions: Some(vec!["php"]),
            },
            // Unsafe deserialization
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:pickle\.loads?|yaml\.load|json\.decode|Marshal\.loads|ObjectInputStream)"#).unwrap(),
                vuln_type: VulnerabilityType::UnsafeDeserialization,
                description: "Unsafe deserialization - can lead to RCE",
                suggestion: "Use safe deserialization formats (JSON) or validate input",
                extensions: None,
            },
            // Missing authentication/authorization
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:@app\.route|@router\.get|@api)\s*\("#).unwrap(),
                vuln_type: VulnerabilityType::MissingAuth,
                description: "API route defined - ensure authentication is applied",
                suggestion: "Ensure proper auth middleware is applied",
                extensions: Some(vec!["py", "js", "ts", "go", "java"]),
            },
            // Insecure random
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)Math\.random\s*\("#).unwrap(),
                vuln_type: VulnerabilityType::InsecureRandom,
                description: "Math.random() is not cryptographically secure",
                suggestion: "Use crypto.randomBytes() or crypto.randomUUID()",
                extensions: Some(vec!["js", "ts"]),
            },
            // Unvalidated redirects
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)(?:redirect|window\.location|response\.sendRedirect)"#).unwrap(),
                vuln_type: VulnerabilityType::UnvalidatedRedirect,
                description: "Redirect with potentially unvalidated URL",
                suggestion: "Validate redirect URLs against an allowlist",
                extensions: None,
            },
            // Buffer overflow (C/C++)
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)\bgets\s*\("#).unwrap(),
                vuln_type: VulnerabilityType::BufferOverflow,
                description: "Use of gets() - no bounds checking, buffer overflow",
                suggestion: "Use fgets() or safer alternatives",
                extensions: Some(vec!["c", "cpp", "cc"]),
            },
            VulnerabilityPattern {
                pattern: Regex::new(r#"(?i)\bstrcpy\s*\(|strcat\s*\("#).unwrap(),
                vuln_type: VulnerabilityType::BufferOverflow,
                description: "Use of strcpy/strcat - potential buffer overflow",
                suggestion: "Use strncpy/strncat with proper size limits",
                extensions: Some(vec!["c", "cpp", "cc"]),
            },
        ];

        Self { patterns }
    }

    /// Scan a single diff for vulnerabilities
    pub fn scan_diff(&self, diff: &ParsedDiff) -> Vec<Vulnerability> {
        let mut vulns = Vec::new();
        let path = diff.new_path.as_ref().or(diff.old_path.as_ref());

        // Check file extension
        let ext = path.and_then(|p| p.rsplit('.').next());

        for hunk in &diff.hunks {
            for (line_num, line) in hunk.lines.iter().enumerate() {
                let line_content = match line {
                    HunkLine::Addition(s) => s,
                    HunkLine::Context(s) => s,
                    HunkLine::Deletion(_) => continue,
                };

                let line_number = hunk.new_start + line_num as u32;

                for pattern in &self.patterns {
                    // Check extension filter
                    if let Some(ref extensions) = pattern.extensions {
                        if let Some(ext) = ext {
                            if !extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }

                    if pattern.pattern.is_match(line_content) {
                        vulns.push(Vulnerability {
                            file: path.cloned().unwrap_or_default(),
                            line: line_number,
                            vuln_type: pattern.vuln_type,
                            description: pattern.description.to_string(),
                            suggestion: Some(pattern.suggestion.to_string()),
                        });
                    }
                }
            }
        }

        vulns
    }

    /// Scan multiple diffs
    pub fn scan_diffs(&self, diffs: &[ParsedDiff]) -> Vec<Vulnerability> {
        diffs.iter().flat_map(|d| self.scan_diff(d)).collect()
    }
}

/// Scan code changes for security vulnerabilities
pub fn scan_for_vulnerabilities(diffs: &[ParsedDiff]) -> Vec<Vulnerability> {
    let scanner = SecurityScanner::new();
    scanner.scan_diffs(diffs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffHunk;

    #[test]
    fn test_detect_hardcoded_password() {
        let scanner = SecurityScanner::new();
        let diff = ParsedDiff {
            old_path: Some("config.py".to_string()),
            new_path: Some("config.py".to_string()),
            is_new: false,
            is_deleted: false,
            is_binary: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![HunkLine::Addition("password = 'supersecret123'".to_string())],
            }],
            language: Some("python".to_string()),
        };

        let vulns = scanner.scan_diff(&diff);
        assert!(!vulns.is_empty());
        assert_eq!(vulns[0].vuln_type, VulnerabilityType::HardcodedCredential);
    }

    #[test]
    fn test_detect_sql_injection() {
        let scanner = SecurityScanner::new();
        let diff = ParsedDiff {
            old_path: Some("db.py".to_string()),
            new_path: Some("db.py".to_string()),
            is_new: false,
            is_deleted: false,
            is_binary: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![HunkLine::Addition("cursor.execute('SELECT * FROM users WHERE id=' + user_id)".to_string())],
            }],
            language: Some("python".to_string()),
        };

        let vulns = scanner.scan_diff(&diff);
        assert!(!vulns.is_empty());
        assert_eq!(vulns[0].vuln_type, VulnerabilityType::SqlInjection);
    }

    #[test]
    fn test_no_false_positives_for_safe_code() {
        let scanner = SecurityScanner::new();
        let diff = ParsedDiff {
            old_path: Some("safe.py".to_string()),
            new_path: Some("safe.py".to_string()),
            is_new: false,
            is_deleted: false,
            is_binary: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![HunkLine::Addition("cursor.execute('SELECT * FROM users WHERE id = ?', [user_id])".to_string())],
            }],
            language: Some("python".to_string()),
        };

        let vulns = scanner.scan_diff(&diff);
        assert!(vulns.is_empty());
    }

    #[test]
    fn test_weak_hash_detection() {
        let scanner = SecurityScanner::new();
        let diff = ParsedDiff {
            old_path: Some("hash.py".to_string()),
            new_path: Some("hash.py".to_string()),
            is_new: false,
            is_deleted: false,
            is_binary: false,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines: vec![HunkLine::Addition("hash = md5(password)".to_string())],
            }],
            language: Some("python".to_string()),
        };

        let vulns = scanner.scan_diff(&diff);
        assert!(!vulns.is_empty());
        assert_eq!(vulns[0].vuln_type, VulnerabilityType::WeakHash);
    }
}
