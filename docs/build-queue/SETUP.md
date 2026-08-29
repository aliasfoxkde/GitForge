# GitForge Build Queue - Quick Start Guide

## Prerequisites

- Rust/Cargo installed
- GitForge repository cloned
- Build artifacts compiled

## Step 1: Build the Components

```bash
cd /nas/Temp/repos/GitForge
cargo build --release
```

## Step 2: Install the Cargo Wrapper

```bash
# From GitForge repository root
./scripts/install-cargo-wrapper.sh

# Restart shell or source bashrc
source ~/.bashrc
```

## Step 3: Start the Daemon

```bash
# Option A: Manual (foreground)
gitforge-buildd

# Option B: Background
gitforge-buildd &
sleep 1

# Option C: Via cargo-wrapper
cargo-wrapper --wrapper-status
```

## Step 4: Verify Installation

```bash
# Check wrapper status
cargo-wrapper --wrapper-status

# Check daemon is running
ls -la target/quality/gitforge-build.sock

# View queue stats
gitforge-build --stats
```

## Step 5: Test Enforcement

```bash
# Test 1: Normal build (should route through daemon)
cargo build --package gitforce-build

# Test 2: No-wait build (returns immediately)
gitforge-build --no-wait -- cargo build --package gitforce-build

# Test 3: Bypass wrapper (should work but without enforcement)
cargo-wrapper --wrapper-fallback -- cargo build --package gitforce-build
```

## Integration Options

### Option A: Cargo Alias (Recommended)

Installs the alias in your shell:
```bash
./scripts/install-cargo-wrapper.sh
source ~/.bashrc
```

### Option B: TaskWizer Integration

The TaskWizer `BuildTool` is already compiled and routes builds through the daemon:

```bash
cd /nas/Temp/repos/taskwizer-rust-cli
cargo build --release
./target/release/taskwizer chat "build the project"
```

### Option C: Claude Code MCP Server

When using Claude Code with the TaskWizer MCP server, the `build` tool is available:

```
Available tools:
- build: Run cargo build/test/check with concurrency control
```

## Common Commands

| Task | Command |
|------|---------|
| Start daemon | `gitforge-buildd &` |
| Check status | `gitforge-build --stats` |
| Submit job | `gitforge-build -- cargo build` |
| Submit and wait | `gitforge-build -- cargo test` |
| Submit no-wait | `gitforge-build --no-wait -- cargo build` |
| Submit managed executable | `gitforge-build --exec scripts/frontend-quality.sh` |
| Bypass daemon | `cargo-wrapper --wrapper-fallback -- <cmd>` |
| View logs | `tail -f target/quality/gitforge-build.log` |

## Troubleshooting

### Daemon won't start
```bash
# Check if socket exists
ls -la target/quality/gitforge-build.sock

# Remove stale socket
rm -f target/quality/gitforge-build.sock

# Try again
gitforge-buildd
```

### Builds hang
```bash
# Check what's running
gitforge-build --stats

# Kill stuck daemon
pkill gitforge-buildd
rm target/quality/gitforge-build.sock

# Restart
gitforge-buildd &
```

For unattended operation, configure admission limits before starting the daemon:

```bash
export GITFORGE_BUILD_MAX_CONCURRENT=8
export GITFORGE_BUILD_MAX_QUEUED=32
export GITFORGE_BUILD_TIMEOUT_SECONDS=3600
export GITFORGE_BUILD_JOURNAL="$XDG_STATE_HOME/gitforge/build/jobs.jsonl"
export GITFORGE_BUILD_MAX_RETAINED_JOBS=4096
gitforge-buildd
```

The queue is bounded so a burst of agent requests cannot create an unbounded
number of worker tasks. A rejected submission is safe to retry after a short
backoff once capacity becomes available.

When a journal is configured, accepted jobs and terminal results are fsynced
as newline-delimited records. Queued jobs are re-enqueued after a daemon
restart; jobs recorded as running are recovered as explicit interrupted
failures because the previous child cannot safely be assumed to be owned.

### Wrapper not working
```bash
# Verify alias exists
alias cargo

# Verify wrapper exists
ls -la ~/.cargo/bin/cargo-wrapper

# Reinstall if needed
./scripts/install-cargo-wrapper.sh
```

## Next Steps

- Read [ENFORCEMENT.md](ENFORCEMENT.md) for detailed architecture
- Set up systemd service for auto-start
- Configure monitoring for production use
- Review backup procedures in [ENFORCEMENT.md](ENFORCEMENT.md#backups)
