# Quality Baseline — Unwrap Inventory (2026-10-10)

Measured by `scripts/unwrap-count.sh` (line-based grep over each crate's
`src/`; includes `#[cfg(test)]` modules — the point is the trend, and test
code legitimately unwraps). This file is the ratchet: a crate's count may
go down or hold, never up. When a crate reaches zero, its
`Cargo.toml` gains the enforcement lints (`clippy::unwrap_used`,
`clippy::expect_used` = "deny" for library crates; "warn" where a binary's
main legitimately expects) under the workspace `[lints]` inheritance.

Per the 2026-10-10 operating directive, CI runs on the GitForge instance
on the fedora remote; the elimination batches are validated there, one
crate-batch per pipeline run.

| crate | unwrap | expect | panic | total |
|---|---:|---:|---:|---:|
| gitforge-ai | 6 | 0 | 0 | 6 |
| gitforge-api | 95 | 33 | 1 | 129 |
| gitforge-build | 29 | 17 | 8 | 54 |
| gitforge-ci | 143 | 2 | 0 | 145 |
| gitforge-cli | 85 | 1 | 0 | 86 |
| gitforge-common | 14 | 0 | 0 | 14 |
| gitforge-core | 78 | 20 | 0 | 98 |
| gitforge-db | 470 | 2 | 0 | 472 |
| gitforge-events | 36 | 1 | 0 | 37 |
| gitforge-process | 7 | 8 | 1 | 16 |
| gitforge-review | 44 | 1 | 0 | 45 |
| gitforge-runner | 64 | 32 | 2 | 98 |
| gitforge-sandbox | 87 | 5 | 0 | 92 |
| gitforge-scheduler | 238 | 7 | 0 | 245 |
| gitforge-storage | 260 | 4 | 0 | 264 |
| api (service) | 15 | 0 | 2 | 17 |
| ci (service) | 222 | 33 | 0 | 255 |
| git-server (service) | 36 | 18 | 3 | 57 |
| runner (service) | 4 | 1 | 0 | 5 |
| **TOTAL** | **1933** | **185** | **17** | **2135** |

## Elimination order (biggest library risk first, service shells last)

1. `gitforge-db` (472) — the durability layer; every unwrap is a potential
   panic inside a write path. Mostly `#[cfg(test)]` setup, so the real
   split comes from `#[cfg(test)]`-aware counting once the crate is
   opened up.
2. `gitforge-storage`, `gitforge-scheduler` — same class.
3. `gitforge-ci`, `gitforge-api`, `gitforge-core`, `gitforge-sandbox`.
4. Services (`ci` first at 255) — binary code, `main()`-adjacent unwraps
   may convert to documented `expect` with context, not silent `unwrap`.

## Related foundation

- Root `[workspace.lints]` (this cycle): `rust_2018_idioms`,
  `unsafe_op_in_unsafe_fn`, `clippy::dbg_macro`,
  `clippy::unimplemented` at warn — safe under the CI `-D warnings`
  gate. Enforcement-grade restriction lints attach per crate with the
  campaign, never workspace-wide ahead of it.
- Coverage ratchet (separate ledger item) requires an in-sandbox
  measurement before raising thresholds; not calibrated this cycle.
