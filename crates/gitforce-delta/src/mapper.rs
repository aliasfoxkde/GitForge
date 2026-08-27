//! Package Mapper — maps file patterns to packages and project types

use glob::Pattern;
use std::collections::HashMap;

/// Project type detected from structure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    /// Go application or service
    GoApp,
    /// Go library
    GoLib,
    /// Python package
    Python,
    /// Rust workspace or binary
    Rust,
    /// TypeScript/Node.js project
    TypeScript,
    /// Mixed or unspecified
    Mixed,
    /// Documentation-only
    Docs,
}

impl ProjectType {
    pub fn from_languages(langs: &[String]) -> Self {
        if langs.is_empty() {
            return ProjectType::Docs;
        }
        if langs.len() == 1 {
            match langs[0].as_str() {
                "go" => return ProjectType::GoApp,
                "python" => return ProjectType::Python,
                "rust" => return ProjectType::Rust,
                "typescript" | "javascript" => return ProjectType::TypeScript,
                "shell" => return ProjectType::Mixed,
                _ => return ProjectType::Mixed,
            }
        }
        ProjectType::Mixed
    }
}

/// A mapping from file pattern glob to package/workflow name
#[derive(Debug, Clone)]
pub struct PackageMapping {
    /// Glob pattern (e.g., "go/**/*.go")
    pub pattern: Pattern,
    /// The package this pattern belongs to
    pub package: String,
    /// Language of this package
    pub language: String,
    /// Whether this package's tests should run if this pattern matches
    pub runs_tests: bool,
}

impl PackageMapping {
    pub fn new(pattern: &str, package: &str, language: &str) -> Self {
        Self {
            pattern: Pattern::new(pattern).unwrap(),
            package: package.to_string(),
            language: language.to_string(),
            runs_tests: true,
        }
    }

    pub fn lib(pattern: &str, package: &str, language: &str) -> Self {
        Self {
            pattern: Pattern::new(pattern).unwrap(),
            package: package.to_string(),
            language: language.to_string(),
            runs_tests: true,
        }
    }
}

/// Maps changed files to packages based on project structure
pub struct PackageMapper {
    mappings: Vec<PackageMapping>,
}

impl Default for PackageMapper {
    fn default() -> Self {
        Self::standard()
    }
}

impl PackageMapper {
    /// Standard mapping for common project structures
    pub fn standard() -> Self {
        let mappings = vec![
            // ─── Go ───────────────────────────────────────────────────────────
            PackageMapping::new("**/go.mod", "go:module", "go"),
            PackageMapping::new("**/go.sum", "go:module", "go"),
            PackageMapping::new("cmd/**/*.go", "go:cmd", "go"),
            PackageMapping::new("internal/**/*.go", "go:internal", "go"),
            PackageMapping::new("pkg/**/*.go", "go:pkg", "go"),
            PackageMapping::new("api/**/*.go", "go:api", "go"),
            PackageMapping::new("services/**/*.go", "go:services", "go"),
            PackageMapping::lib("**/*.go", "go:.", "go"),
            // ─── Python ──────────────────────────────────────────────────────
            PackageMapping::new("pyproject.toml", "python:module", "python"),
            PackageMapping::new("setup.py", "python:module", "python"),
            PackageMapping::new("src/**/*.py", "python:src", "python"),
            PackageMapping::new("tests/**/*.py", "python:tests", "python"),
            PackageMapping::lib("**/*.py", "python:.", "python"),
            // ─── Rust ────────────────────────────────────────────────────────
            PackageMapping::new("Cargo.toml", "rust:workspace", "rust"),
            PackageMapping::new("Cargo.lock", "rust:workspace", "rust"),
            PackageMapping::new("crates/*/Cargo.toml", "rust:crate", "rust"),
            PackageMapping::new("services/*/Cargo.toml", "rust:service", "rust"),
            PackageMapping::lib("**/*.rs", "rust:.", "rust"),
            // ─── TypeScript / Node ──────────────────────────────────────────
            PackageMapping::new("package.json", "ts:module", "typescript"),
            PackageMapping::new("tsconfig.json", "ts:config", "typescript"),
            PackageMapping::new("src/**/*.ts", "ts:src", "typescript"),
            PackageMapping::new("src/**/*.tsx", "ts:src", "typescript"),
            PackageMapping::new("tests/**/*.ts", "ts:tests", "typescript"),
            PackageMapping::new("tests/**/*.tsx", "ts:tests", "typescript"),
            PackageMapping::lib("**/*.{ts,tsx,js,jsx}", "ts:.", "typescript"),
            // ─── Shell ───────────────────────────────────────────────────────
            PackageMapping::new("scripts/**/*.sh", "shell:scripts", "shell"),
            PackageMapping::new(".githooks/**/*.sh", "shell:hooks", "shell"),
            PackageMapping::lib("**/*.sh", "shell:.", "shell"),
            // ─── Docs ───────────────────────────────────────────────────────
            PackageMapping::new("docs/**/*.md", "docs", "markdown"),
            PackageMapping::new("README.md", "docs", "markdown"),
            PackageMapping::new("CHANGELOG.md", "docs", "markdown"),
            // ─── CI/CD ───────────────────────────────────────────────────────
            PackageMapping::new(".github/workflows/*.yml", "ci:workflows", "yaml"),
            PackageMapping::new(".github/workflows/*.yaml", "ci:workflows", "yaml"),
            PackageMapping::new(".github/actions/**/*", "ci:actions", "yaml"),
            PackageMapping::new(".githooks/*", "ci:hooks", "shell"),
            PackageMapping::new(".githooks/*/", "ci:hooks", "shell"),
            PackageMapping::new("Makefile", "ci:makefile", "shell"),
            PackageMapping::new("**/Dockerfile*", "ci:docker", "dockerfile"),
            // ─── Config ──────────────────────────────────────────────────────
            PackageMapping::new("*.toml", "config", "toml"),
            PackageMapping::new("*.yaml", "config", "yaml"),
            PackageMapping::new("*.yml", "config", "yaml"),
            PackageMapping::new("*.json", "config", "json"),
            PackageMapping::new(".golangci.yml", "config:go-linter", "yaml"),
            PackageMapping::new(".ruff.toml", "config:python-linter", "toml"),
        ];

        Self { mappings }
    }

    /// Map a single changed file path to its package
    pub fn map_file(&self, file_path: &str) -> Option<&PackageMapping> {
        self.mappings.iter().find(|m| m.pattern.matches(file_path))
    }

    /// Map multiple files and return unique affected packages
    pub fn map_files<'a>(&'a self, files: &'a [String]) -> Vec<&'a PackageMapping> {
        let mut seen: HashMap<String, &PackageMapping> = HashMap::new();
        for file in files {
            if let Some(mapping) = self.map_file(file) {
                // Keep the most specific (first matching) mapping per package
                seen.entry(mapping.package.clone()).or_insert(mapping);
            }
        }
        seen.into_values().collect()
    }

    /// Given affected packages, return which workflow files should run
    pub fn workflows_for_packages(&self, packages: &[String]) -> Vec<String> {
        let mut workflows = vec![];

        for pkg in packages {
            if pkg.starts_with("go:") {
                workflows.push("ci.yml".to_string());
                workflows.push("lint.yml".to_string());
            } else if pkg.starts_with("python:") {
                workflows.push("python-ci.yml".to_string());
            } else if pkg.starts_with("rust:") {
                workflows.push("rust.yml".to_string());
            } else if pkg.starts_with("ts:") {
                workflows.push("ci.yml".to_string());
            } else if pkg == "ci:workflows" || pkg == "ci:hooks" {
                workflows.push("ci.yml".to_string());
                workflows.push("always.yml".to_string());
            } else if pkg == "docs" {
                workflows.push("smoke-test.yml".to_string());
            }
        }

        workflows.sort();
        workflows.dedup();
        workflows
    }
}
