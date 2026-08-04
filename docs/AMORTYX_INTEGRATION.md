# Amortyx + GitForge Integration Guide

**Purpose**: Build Windows release binaries for Amortyx MCP Server using GitForge CI/CD
**Date**: 2026-07-21

---

## Current Status

| Component | Status | Notes |
|-----------|--------|-------|
| GitForge workspace | ✅ | Builds pass, 872+ tests, 89.65% coverage |
| Windows cross-compile | ✅ | `cross build --release --target x86_64-pc-windows-gnu --bin gitforge` works |
| Linux native build | ✅ | `cargo build --release --bin gitforge` works |
| macOS cross-compile | ❌ | Requires macOS runner or hardware |
| Services (api/ci/etc) | ⚠️ | Linux-only (Unix signals) |

---

## Quick Start: Building Windows Binaries

### On this system (Linux with cross tool)

```bash
# Build gitforge CLI for Windows
cross build --release --target x86_64-pc-windows-gnu --bin gitforge

# Output: target/x86_64-pc-windows-gnu/release/gitforge.exe (3.3MB)
```

### GitHub Actions (when you hit free tier limits)

Create `.github/workflows/build-windows.yml`:

```yaml
name: Build Windows

on:
  push:
    tags: ['v*']
  workflow_dispatch:

jobs:
  build-windows:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Install cross
        uses: taiki-e/install-action@cross

      - name: Build for Windows
        run: cross build --release --target x86_64-pc-windows-gnu --bin gitforge

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: windows-binaries
          path: target/x86_64-pc-windows-gnu/release/gitforge.exe
```

---

## GitForge Services Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      GitForge                                │
├─────────────┬─────────────┬─────────────┬──────────────────┤
│  git-server │     api     │     ci     │     runner       │
│  (port 22)  │  (port     │  (event    │  (job            │
│  SSH/HTTP   │   8080)     │  consumer) │  executor)       │
└─────────────┴─────────────┴─────────────┴──────────────────┘
      │              │              │              │
      └──────────────┴──────────────┴──────────────┘
                         │
                   InMemoryEventBus
                   (local message bus)
```

**NOTE**: All services use Unix signals for graceful shutdown → **Linux only**

---

## GitForge CLI Commands (gitforge.exe)

```
gitforge --help
Commands:
  auth        # Authentication (login/logout)
  repo       # Repository management (create, list, clone, push, pull)
  pipeline   # CI/CD pipeline management
  runner     # Runner agent management
  sync       # Cloud sync (push/pull state)
```

---

## For Amortyx MCP Server Integration

Since Amortyx needs **Windows builds** and GitForge's services are Linux-only:

### Option 1: Self-hosted GitForge on VPS

1. Deploy GitForge services on a Linux VPS
2. Use GitForge CLI to register runners
3. Amortyx pushes to GitForge, runners execute on Linux

### Option 2: Hybrid Approach

1. Keep using GitHub Actions for CI/CD (if you have free tier)
2. Use GitForge for:
   - Repository hosting (git-server)
   - Artifact storage (gitforce-storage)
   - Runner management

### Option 3: Build GitForge on Mac Mini

When you set up the Mac Mini for macOS builds:

```bash
# On macOS
./scripts/cross-build.sh mac    # Builds for macOS x86_64 + ARM64
./scripts/cross-build.sh all    # Linux + Windows + macOS
```

---

## Environment Variables for GitForge

```bash
# API Gateway
JWT_SECRET="your-secret-here"
PORT=8080
DATABASE_URL="sqlite:/gitforge.db"

# Git Server
GIT_ROOT="/var/lib/gitforge/repos"

# Runner Agent
RUNNER_API_URL="http://localhost:42780"
RUNNER_TOKEN="runner-registration-token"

# Logging
RUST_LOG=info  # trace, debug, info, warn, error
```

---

## Validation Commands

```bash
# Build check
cargo build --workspace        # All binaries
cargo test --workspace         # 872+ tests

# Coverage check
cargo llvm-cov report          # Should show 89.65%

# Lint check
cargo clippy --all-targets --all-features -- -D warnings

# Health check (when services running)
curl http://localhost:42780/health
curl http://localhost:42780/metrics
```

---

## Files to Share with Amortyx Agent

1. `docs/AMORTYX_INTEGRATION.md` - This file
2. `docs/MACOS_BUILD.md` - macOS build plan
3. `Makefile` - Build targets
4. `Cross.toml` - Cross-compilation config

---

## Next Steps for 99% Coverage Goal

Remaining coverage gaps:
1. **CLI sync push/pull** - Requires actual HTTP server
2. **Service main() entry points** - tokio::main can't be tested directly
3. **get_job_logs** - Placeholder implementation

For the Amortyx MCP server build integration, the Windows binary build pipeline is ready to use.
