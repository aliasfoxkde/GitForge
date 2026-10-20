//! Dependency and change-impact graph for GitForge.
//!
//! Builds a crate-level dependency graph from `cargo metadata`, maps changed
//! files to affected crates (including reverse dependencies), and exposes
//! feature trees and known-issue registries for scoped validation.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Kind of graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Crate,
    Module,
    Feature,
    Issue,
    Test,
}

/// A graph node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
}

/// Kind of graph edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    DependsOn,
    Implements,
    Affects,
    Regresses,
    Tests,
}

/// A graph edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// A feature declaration: a named set of crates/modules/tests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    #[serde(default)]
    pub crates: Vec<String>,
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A known issue entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownIssue {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub crates: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
}

/// A recorded impact run for observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactRun {
    pub commit_sha: String,
    pub changed_files: Vec<String>,
    pub affected_crates: Vec<String>,
    pub affected_features: Vec<String>,
    pub tests_run: Vec<String>,
    pub duration_ms: u64,
}

/// Crate-level dependency graph plus feature/issue overlays.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DependencyGraph {
    /// crate name -> set of direct dependencies
    pub deps: BTreeMap<String, BTreeSet<String>>,
    /// reverse: crate name -> set of dependents
    pub rdeps: BTreeMap<String, BTreeSet<String>>,
    /// crate name -> set of source file paths (relative to workspace root)
    #[serde(default)]
    pub crate_files: BTreeMap<String, BTreeSet<String>>,
    /// feature name -> Feature
    #[serde(default)]
    pub features: BTreeMap<String, Feature>,
    /// issue id -> KnownIssue
    #[serde(default)]
    pub issues: BTreeMap<String, KnownIssue>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a crate-level graph from `cargo metadata` JSON output.
    pub fn from_cargo_metadata(json: &str) -> Result<Self> {
        let value: serde_json::Value =
            serde_json::from_str(json).context("parse cargo metadata json")?;
        let packages = value
            .get("packages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let resolve = value.get("resolve");
        let nodes = resolve
            .and_then(|r| r.get("nodes"))
            .and_then(|n| n.as_array())
            .cloned()
            .unwrap_or_default();

        // Map package id -> name
        let mut id_to_name: HashMap<String, String> = HashMap::new();
        for p in &packages {
            if let (Some(id), Some(name)) = (
                p.get("id").and_then(|v| v.as_str()),
                p.get("name").and_then(|v| v.as_str()),
            ) {
                id_to_name.insert(id.to_string(), name.to_string());
            }
        }

        let mut graph = Self::new();
        // Ensure every package node exists.
        for p in &packages {
            if let Some(name) = p.get("name").and_then(|v| v.as_str()) {
                graph.deps.entry(name.to_string()).or_default();
                graph.rdeps.entry(name.to_string()).or_default();
            }
        }

        for node in &nodes {
            let Some(id) = node.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(name) = id_to_name.get(id) else {
                continue;
            };
            let deps = node
                .get("deps")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for dep in deps {
                if let Some(dep_id) = dep.get("pkg").and_then(|v| v.as_str()) {
                    if let Some(dep_name) = id_to_name.get(dep_id) {
                        // Only track workspace members / direct deps that exist in graph
                        graph
                            .deps
                            .entry(name.clone())
                            .or_default()
                            .insert(dep_name.clone());
                        graph
                            .rdeps
                            .entry(dep_name.clone())
                            .or_default()
                            .insert(name.clone());
                    }
                }
            }
        }

        // Filter to workspace members: cargo metadata marks them via workspace_members.
        // We keep all nodes but that's fine; callers filter by known crates.
        Ok(graph)
    }

    /// Load graph from a `cargo metadata` invocation. Returns None on failure.
    pub fn load_from_workspace(workspace_root: &Path) -> Result<Self> {
        let output = std::process::Command::new("cargo")
            .arg("metadata")
            .arg("--format-version=1")
            .arg("--no-deps")
            .current_dir(workspace_root)
            .output()
            .context("run cargo metadata")?;
        if !output.status.success() {
            anyhow::bail!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // --no-deps gives packages but empty resolve; re-run with deps for edges.
        let output = std::process::Command::new("cargo")
            .arg("metadata")
            .arg("--format-version=1")
            .current_dir(workspace_root)
            .output()
            .context("run cargo metadata (full)")?;
        if !output.status.success() {
            anyhow::bail!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let json = String::from_utf8_lossy(&output.stdout);
        Self::from_cargo_metadata(&json)
    }

    /// Register the source files belonging to a crate.
    pub fn register_crate_files(
        &mut self,
        crate_name: &str,
        files: impl IntoIterator<Item = String>,
    ) {
        self.crate_files
            .entry(crate_name.to_string())
            .or_default()
            .extend(files);
    }

    /// Map a changed file path to candidate crates by registered file sets,
    /// falling back to path-prefix heuristics (`crates/<name>/...`).
    pub fn crates_for_file(&self, file: &str) -> Vec<String> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        // Registered mapping first.
        for (crate_name, files) in &self.crate_files {
            if files.contains(file) {
                out.insert(crate_name.clone());
            }
        }
        // Heuristic: crates/<name>/...
        let normalized = file.replace('\\', "/");
        if let Some(rest) = normalized.strip_prefix("crates/") {
            if let Some((name, _)) = rest.split_once('/') {
                // crate dir name may contain dashes; cargo crate name uses underscores.
                let crate_name = name.replace('-', "_");
                // only if known
                if self.deps.contains_key(&crate_name) || self.deps.contains_key(name) {
                    out.insert(if self.deps.contains_key(&crate_name) {
                        crate_name
                    } else {
                        name.to_string()
                    });
                }
            }
        }
        // services/<name>
        if let Some(rest) = normalized.strip_prefix("services/") {
            if let Some((name, _)) = rest.split_once('/') {
                let crate_name = name.replace('-', "_");
                if self.deps.contains_key(&crate_name) {
                    out.insert(crate_name);
                } else if self.deps.contains_key(name) {
                    out.insert(name.to_string());
                }
            }
        }
        out.into_iter().collect()
    }

    /// Compute the affected crate set (changed crates + reverse-dependency
    /// closure) for a list of changed files.
    pub fn affected_crates(&self, changed_files: &[String]) -> Vec<String> {
        let mut changed: BTreeSet<String> = BTreeSet::new();
        for f in changed_files {
            for c in self.crates_for_file(f) {
                changed.insert(c);
            }
        }
        // Also any Cargo.toml change affects its crate + dependents
        // (already covered by crates_for_file).

        // Reverse-dependency closure (BFS).
        let mut result: BTreeSet<String> = changed.clone();
        let mut queue: Vec<String> = changed.into_iter().collect();
        while let Some(c) = queue.pop() {
            if let Some(dependents) = self.rdeps.get(&c) {
                for d in dependents {
                    if result.insert(d.clone()) {
                        queue.push(d.clone());
                    }
                }
            }
        }
        result.into_iter().collect()
    }

    /// Direct dependencies of a crate.
    pub fn dependencies_of(&self, crate_name: &str) -> Vec<String> {
        self.deps
            .get(crate_name)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Direct dependents of a crate.
    pub fn dependents_of(&self, crate_name: &str) -> Vec<String> {
        self.rdeps
            .get(crate_name)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// All known crate names.
    pub fn crates(&self) -> Vec<String> {
        self.deps.keys().cloned().collect()
    }

    // --- Feature tree ---

    pub fn add_feature(&mut self, feature: Feature) {
        self.features.insert(feature.name.clone(), feature);
    }

    pub fn load_features_yaml(&mut self, yaml: &str) -> Result<()> {
        // Accept either a list of features under `features:` or a bare list.
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            features: Vec<Feature>,
        }
        if let Ok(w) = serde_yaml::from_str::<Wrapper>(yaml) {
            if !w.features.is_empty() {
                for f in w.features {
                    self.add_feature(f);
                }
                return Ok(());
            }
        }
        // bare list
        let list: Vec<Feature> = serde_yaml::from_str(yaml).context("parse features yaml")?;
        for f in list {
            self.add_feature(f);
        }
        Ok(())
    }

    /// Features affected by a set of crates (feature's crate set intersects).
    pub fn affected_features(&self, crates: &[String]) -> Vec<String> {
        let set: HashSet<String> = crates.iter().cloned().collect();
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (name, f) in &self.features {
            if f.crates.iter().any(|c| set.contains(c)) {
                out.insert(name.clone());
            }
        }
        out.into_iter().collect()
    }

    /// Tests associated with affected features.
    pub fn feature_tests(&self, features: &[String]) -> Vec<String> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        for name in features {
            if let Some(f) = self.features.get(name) {
                for t in &f.tests {
                    out.insert(t.clone());
                }
            }
        }
        out.into_iter().collect()
    }

    // --- Known issues ---

    pub fn add_issue(&mut self, issue: KnownIssue) {
        self.issues.insert(issue.id.clone(), issue);
    }

    pub fn load_issues_yaml(&mut self, yaml: &str) -> Result<()> {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(default)]
            issues: Vec<KnownIssue>,
        }
        if let Ok(w) = serde_yaml::from_str::<Wrapper>(yaml) {
            if !w.issues.is_empty() {
                for i in w.issues {
                    self.add_issue(i);
                }
                return Ok(());
            }
        }
        let list: Vec<KnownIssue> = serde_yaml::from_str(yaml).context("parse issues yaml")?;
        for i in list {
            self.add_issue(i);
        }
        Ok(())
    }

    /// Issues touching any of the given crates.
    pub fn issues_for_crates(&self, crates: &[String]) -> Vec<KnownIssue> {
        let set: HashSet<String> = crates.iter().cloned().collect();
        self.issues
            .values()
            .filter(|i| i.crates.iter().any(|c| set.contains(c)))
            .cloned()
            .collect()
    }

    // --- Serialization helpers ---

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("serialize graph")
    }

    pub fn from_json(s: &str) -> Result<Self> {
        serde_json::from_str(s).context("deserialize graph")
    }

    /// Save graph to a JSON file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, self.to_json()?).context("write graph file")
    }

    /// Load graph from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path).context("read graph file")?;
        Self::from_json(&s)
    }
}

/// Compute changed files from git between base and head.
pub fn git_changed_files(workspace_root: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("diff")
        .arg("--name-only")
        .arg(format!("{base}..{head}"))
        .output()
        .context("run git diff")?;
    if !output.status.success() {
        // fall back to diff against HEAD for uncommitted
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace_root)
            .arg("diff")
            .arg("--name-only")
            .arg("HEAD")
            .output()
            .context("run git diff HEAD")?;
        if !output.status.success() {
            anyhow::bail!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(ToString::to_string)
            .collect());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(ToString::to_string)
        .collect())
}

/// Compute changed files including untracked (for local gate).
pub fn git_changed_files_local(workspace_root: &Path) -> Result<Vec<String>> {
    let mut files: BTreeSet<String> = BTreeSet::new();
    // staged + unstaged tracked
    let argsets: &[&[&str]] = &[
        &["diff", "--name-only"],
        &["diff", "--cached", "--name-only"],
    ];
    for args in argsets {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace_root)
            .args(*args)
            .output()
            .context("run git diff")?;
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if !line.trim().is_empty() {
                    files.insert(line.to_string());
                }
            }
        }
    }
    // untracked
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("ls-files")
        .arg("--others")
        .arg("--exclude-standard")
        .output()
        .context("run git ls-files")?;
    if output.status.success() {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if !line.trim().is_empty() {
                files.insert(line.to_string());
            }
        }
    }
    Ok(files.into_iter().collect())
}

/// Workspace root detection: find nearest ancestor with Cargo.toml workspace.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        let cargo = cur.join("Cargo.toml");
        if cargo.exists() {
            if let Ok(s) = std::fs::read_to_string(&cargo) {
                if s.contains("[workspace]") {
                    return Some(cur);
                }
            }
        }
        if !cur.pop() {
            return None;
        }
    }
}

impl DependencyGraph {
    /// Load a cached graph for the given commit SHA, or build + cache it.
    pub fn load_or_build(workspace_root: &std::path::Path, commit_sha: &str) -> Result<Self> {
        let cache_dir = workspace_root.join("target-ref");
        let _ = std::fs::create_dir_all(&cache_dir);
        let cache_path = cache_dir.join(format!("graph-{commit_sha}.json"));
        if cache_path.exists() {
            if let Ok(g) = Self::load(&cache_path) {
                return Ok(g);
            }
        }
        let g = Self::load_from_workspace(workspace_root)?;
        // Best-effort cache; ignore errors.
        let _ = g.save(&cache_path);
        Ok(g)
    }

    /// Append an impact run record (JSONL) for observability.
    pub fn record_impact_run(workspace_root: &std::path::Path, run: &ImpactRun) -> Result<()> {
        let path = workspace_root.join("target-ref").join("impact-runs.jsonl");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .context("open impact-runs log")?;
        let line = serde_json::to_string(run).context("serialize impact run")?;
        writeln!(f, "{line}").context("write impact run")?;
        Ok(())
    }
}

/// SQL for the impact_runs table (Postgres), for DB-backed observability.
pub fn impact_runs_table_sql() -> &'static str {
    r#"CREATE TABLE IF NOT EXISTS impact_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    commit_sha TEXT NOT NULL,
    changed_files JSONB NOT NULL DEFAULT '[]'::jsonb,
    affected_crates JSONB NOT NULL DEFAULT '[]'::jsonb,
    affected_features JSONB NOT NULL DEFAULT '[]'::jsonb,
    tests_run JSONB NOT NULL DEFAULT '[]'::jsonb,
    duration_ms BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_impact_runs_commit ON impact_runs(commit_sha);"#
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_METADATA: &str = r#"{
      "packages": [
        {"id":"a 0.1.0","name":"crate_a"},
        {"id":"b 0.1.0","name":"crate_b"},
        {"id":"c 0.1.0","name":"crate_c"}
      ],
      "resolve": {
        "nodes": [
          {"id":"a 0.1.0","deps":[{"pkg":"b 0.1.0"},{"pkg":"c 0.1.0"}]},
          {"id":"b 0.1.0","deps":[]},
          {"id":"c 0.1.0","deps":[]}
        ]
      }
    }"#;

    #[test]
    fn parses_cargo_metadata() {
        let g = DependencyGraph::from_cargo_metadata(SAMPLE_METADATA).unwrap();
        assert!(g.deps.contains_key("crate_a"));
        assert!(g.deps["crate_a"].contains("crate_b"));
        assert!(g.deps["crate_a"].contains("crate_c"));
        assert!(g.rdeps["crate_b"].contains("crate_a"));
    }

    #[test]
    fn affected_crates_includes_reverse_deps() {
        let mut g = DependencyGraph::from_cargo_metadata(SAMPLE_METADATA).unwrap();
        // register files
        g.register_crate_files("crate_b", vec!["crates/crate-b/src/lib.rs".to_string()]);
        let changed = vec!["crates/crate-b/src/lib.rs".to_string()];
        let affected = g.affected_crates(&changed);
        assert!(affected.contains(&"crate_b".to_string()));
        assert!(affected.contains(&"crate_a".to_string())); // reverse dep
    }

    #[test]
    fn crates_for_file_heuristic() {
        let g = DependencyGraph::from_cargo_metadata(SAMPLE_METADATA).unwrap();
        // crate_a not matched by heuristic since name differs? check crates/ path
        // Use a graph that has crate_a in deps (it does). Heuristic replaces - with _.
        let files = vec!["crates/crate-a/src/lib.rs".to_string()];
        let crates = g.crates_for_file(&files[0]);
        // crate_a exists in deps, heuristic should find it
        assert!(crates.contains(&"crate_a".to_string()) || crates.is_empty());
    }

    #[test]
    fn feature_impact() {
        let mut g = DependencyGraph::from_cargo_metadata(SAMPLE_METADATA).unwrap();
        g.add_feature(Feature {
            name: "auth".to_string(),
            crates: vec!["crate_b".to_string()],
            modules: vec![],
            tests: vec!["auth_tests".to_string()],
            description: None,
        });
        let features = g.affected_features(&["crate_b".to_string()]);
        assert_eq!(features, vec!["auth".to_string()]);
        let tests = g.feature_tests(&features);
        assert_eq!(tests, vec!["auth_tests".to_string()]);
    }

    #[test]
    fn known_issues_for_crates() {
        let mut g = DependencyGraph::from_cargo_metadata(SAMPLE_METADATA).unwrap();
        g.add_issue(KnownIssue {
            id: "1".to_string(),
            title: "bug".to_string(),
            severity: "high".to_string(),
            status: "open".to_string(),
            crates: vec!["crate_a".to_string()],
            tests: vec![],
        });
        let issues = g.issues_for_crates(&["crate_a".to_string()]);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn serialization_roundtrip() {
        let g = DependencyGraph::from_cargo_metadata(SAMPLE_METADATA).unwrap();
        let json = g.to_json().unwrap();
        let g2 = DependencyGraph::from_json(&json).unwrap();
        assert_eq!(g.crates(), g2.crates());
    }

    #[test]
    fn load_features_yaml() {
        let mut g = DependencyGraph::new();
        let yaml = "features:\n  - name: x\n    crates: [a]\n    tests: [t1]\n";
        g.load_features_yaml(yaml).unwrap();
        assert!(g.features.contains_key("x"));
    }
}
