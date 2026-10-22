# Impact Graph

GitForge's dependency and change-impact graph lives in the `gitforge-graph` crate. It maps code → crates → features → tests and powers change-scoped validation.

## Node kinds

- **Crate** — a workspace member (from `cargo metadata`).
- **Module / function** — (planned) symbol-level nodes from codebase-memory-mcp.
- **Feature** — declared in `docs/planning/FEATURES.yml`.
- **Issue** — known issue from GitHub or the F-series ledger (`docs/planning/KNOWN_ISSUES.yml`).
- **Test** — test name linked to a feature.

## Edges

`DependsOn`, `Implements`, `Affects`, `Regresses`, `Tests`.

## Usage

```bash
# Build graph from workspace
cargo run -p gitforge-cli -- check --fast --changed
```

Programmatic:

```rust
use gitforge_graph::{DependencyGraph, find_workspace_root};
let root = find_workspace_root(&std::env::current_dir().unwrap()).unwrap();
let graph = DependencyGraph::load_from_workspace(&root).unwrap();
let changed = vec!["crates/gitforge-graph/src/lib.rs".to_string()];
let affected = graph.affected_crates(&changed); // includes reverse deps
let features = graph.affected_features(&affected);
```

## Fail-open policy

If graph computation fails, validation escalates to a full workspace run; it never skips checks. The graph scopes work, it does not gate verdicts.

## Files

- `docs/planning/FEATURES.yml` — feature declarations.
- `docs/planning/KNOWN_ISSUES.yml` — known issue registry.
