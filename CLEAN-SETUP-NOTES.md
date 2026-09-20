# GitForge clean-setup notes — t5500 audit branch (2026-09-20)

## Build
- `cargo build --release` clean: **3m** on 12 threads (Westmere-safe).

## Runtime topology (discovered — differs from config.toml.example)
- The deployable unit is **`git-server`** (standalone, env-driven):
  starts on :42782 by default, git root = `./gitforge-repos` under the
  working directory, runs WITHOUT a database when `DATABASE_URL` is
  unset, and warns (non-fatal) when `GITFORGE_CI_TRIGGER_URL/TOKEN` are
  unset — pushes then simply don't trigger CI.
- `config.toml.example` describes the wider forge stack (API server
  :8080, SQLite/Postgres, JWT) — a doc gap: nothing on the README path
  explains that `git-server` alone gives a working Git HTTP endpoint
  without any config file.
- `gitforge` (the CLI) is a client: auth/admin/repo/pipeline/runner/sync.

## Gaps found
1. README quick-start doesn't mention `git-server`'s env contract
   (`GITFORGE_GIT_ROOT`-style root override is undocumented — we used
   WorkingDirectory instead).
2. The env var names for git root + CI trigger should be listed in
   `--help` output (currently silent).
3. config.toml.example's [auth] jwt_secret guidance is good; git-server's
   "running without database" mode should warn that auth is off.

## Verified working
- git-server active under systemd (User=, WorkingDirectory=/data/gitforge,
  MemoryMax 2G) — listening on 0.0.0.0:42782.
