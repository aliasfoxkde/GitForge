# GitForge Build Queue - Enforcement & Best Practices

## Overview

GitForge Build Queue (`gitforge-buildd`) is a centralized build coordination system that prevents system overload from concurrent cargo builds. It enforces a maximum of **8 concurrent jobs** and **32 additional queued jobs** by default via bounded admission control while keeping submissions queued and responsive.

The daemon accepts these bounded environment overrides:

| Variable | Default | Allowed range | Purpose |
|---|---:|---:|---|
| `GITFORGE_BUILD_MAX_CONCURRENT` | `8` | `1..64` | Child processes allowed to execute concurrently |
| `GITFORGE_BUILD_MAX_QUEUED` | `32` | `0..1024` | Additional jobs admitted while workers are busy |
| `GITFORGE_BUILD_TIMEOUT_SECONDS` | `3600` | `1..86400` | Wall-clock limit for each child process |

Values outside the allowed range are clamped. A full queue returns a protocol error immediately; callers should retry with backoff rather than repeatedly spawning local workers.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Claude Code Agent                                           │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ MCP Server  │  │ BuildTool   │  │ cargo-wrapper alias │  │
│  │ (build tool)│  │ (TaskWizer) │  │ (bash alias)       │  │
│  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
└─────────┼────────────────┼───────────────────┼───────────────┘
          │                │                   │
          ▼                ▼                   ▼
┌─────────────────────────────────────────────────────────────┐
│              gitforge-build CLI                             │
│         (submits jobs to daemon via Unix socket)           │
└─────────────────────────────┬───────────────────────────────┘
                              │
                              ▼ Unix Socket: /tmp/gitforge-build.sock
┌─────────────────────────────────────────────────────────────┐
│                   gitforge-buildd daemon                     │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │  Semaphore │  │   SIGCHLD   │  │   Subreaper         │  │
│  │  (max: 2)  │  │   Handler   │  │   (reaps zombies)   │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│                              │
│              ┌───────────────┼───────────────┐
│              ▼               ▼               ▼
│         ┌────────┐      ┌────────┐      ┌────────┐
│         │ cargo │      │ cargo │      │ cargo │
│         │ build │      │ test  │      │ check │
│         └────────┘      └────────┘      └────────┘
└─────────────────────────────────────────────────────────────┘
```

## Enforcement Mechanisms

### 1. Cargo Wrapper Alias (RECOMMENDED - Safe)

**How it works:**
- `alias cargo=~/.cargo/bin/cargo-wrapper` in shell
- `cargo-wrapper` routes commands through `gitforge-build` CLI
- Falls back to real cargo if daemon not running

**Pros:**
- ✅ Non-invasive - easy to disable
- ✅ No binary replacement
- ✅ Graceful fallback

**Cons:**
- ❌ Only works in shells with the alias
- ❌ Claude Code may not source shell RC

**Setup:**
```bash
# From GitForge repository root
./scripts/install-cargo-wrapper.sh
source ~/.bashrc
```

### 2. MCP Server Build Tool (For Claude Code)

**How it works:**
- Claude Code calls `build` tool via MCP protocol
- MCP server routes to `gitforge-build` CLI
- Enforces at Claude Code tool level

**Pros:**
- ✅ Integrated into Claude Code workflow
- ✅ No shell modification needed

**Cons:**
- ❌ Claude Code can choose not to use the tool
- ❌ Only works when using TaskWizer MCP server

### 3. TaskWizer BuildTool (For TaskWizer Agents)

**How it works:**
- TaskWizer agents use `BuildTool` handler
- Routes via Unix socket to `gitforge-buildd`
- Enforced at agent framework level

**Pros:**
- ✅ Built into TaskWizer tool registry
- ✅ Direct socket communication

**Cons:**
- ❌ Only for TaskWizer-based agents

## Bypass Methods (Known)

| Method | Can Bypass? | How to Block |
|--------|-------------|--------------|
| Direct `cargo` call | ✅ Yes | Shell alias + path ordering |
| Full path `/usr/bin/cargo` | ✅ Yes | SELinux/AppArmor profile |
| `cargo-real` rename trick | ✅ Yes | Process restrictions |
| Claude Code ignoring MCP tool | ✅ Yes | Agent instructions |

## Best Practices for Agentic Systems

### 1. Install the Alias (Required for可靠 enforcement)

```bash
# Install during agent setup
./scripts/install-cargo-wrapper.sh
source ~/.bashrc

# Verify installation
cargo-wrapper --wrapper-status
```

### 2. Set Up Process Limits

For Claude Code agents, add to system or user limits:

```bash
# /etc/security/limits.conf or ~/.limitsrc
mkinney    soft    nproc   512
mkinney    hard    nproc   1024
```

### 3. Configure Daemon Autostart

```bash
# systemd user service: ~/.config/systemd/user/gitforge-buildd.service
[Unit]
Description=GitForge Build Daemon
After=network.target

[Service]
ExecStart=/path/to/gitforge-buildd
Environment=GITFORGE_BUILD_MAX_CONCURRENT=8
Environment=GITFORGE_BUILD_MAX_QUEUED=32
Environment=GITFORGE_BUILD_TIMEOUT_SECONDS=3600
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```bash
systemctl --user enable gitforge-buildd
systemctl --user start gitforge-buildd
```

### 4. Monitor Queue Health

```bash
# Add to crontab for health checks
*/5 * * * * /path/to/gitforge-build --stats >> /var/log/build-queue.log
```

### 5. Set Claude Code Instructions

In `.claude/CLAUDE.md` or project settings:

```markdown
# Build Enforcement

All cargo builds MUST route through gitforge-build. Non-Cargo automation may
use the explicit `--exec PROGRAM` path so the same queue, timeout, process
group, cancellation, and output containment apply; the executable is never
invoked through a shell unless the caller explicitly selects a shell.
- Use `cargo build` → routes through cargo-wrapper alias
- Use `cargo test` → routes through cargo-wrapper alias
- NEVER call cargo directly without the wrapper

If gitforge-buildd is not running:
1. Start it: `gitforge-buildd`
2. Wait: `sleep 1`
3. Retry your build command
```

## Troubleshooting

### "Daemon not running" errors

```bash
# Check if daemon is running
ls -la /tmp/gitforge-build.sock

# Start daemon
gitforge-buildd &

# Or use cargo-wrapper which auto-falls-back
cargo build  # Falls back to real cargo if daemon down
```

### Jobs stuck in queue

```bash
# Check queue status
gitforge-build --stats

# If hung, kill and restart daemon
pkill gitforge-buildd
rm /tmp/gitforge-build.sock
gitforge-buildd &
```

### Zombie processes appearing

The daemon owns and waits for every child it starts; it does not run a global
SIGCHLD reaper because that races with Tokio. If zombies appear:

```bash
# Check daemon is functioning
ps aux | grep gitforge-buildd

# Check daemon and child states
cat /proc/$(pgrep gitforge-buildd)/status | grep -i sig
```

## Security Considerations

1. **Socket permissions**: The Unix socket should be restricted:
   ```bash
   chmod 600 /tmp/gitforge-build.sock
   chown mkinney:mkinney /tmp/gitforge-build.sock
   ```

2. **No sudo builds in daemon**: Run daemon as normal user, not root

3. **Resource limits**: Configure ulimits before starting daemon:
   ```bash
   ulimit -u 512  # max processes
   gitforge-buildd
   ```

## Quick Reference

| Task | Command |
|------|---------|
| Install wrapper | `./scripts/install-cargo-wrapper.sh` |
| Check status | `cargo-wrapper --wrapper-status` |
| View queue | `gitforge-build --stats` |
| Cancel a job | `gitforge-build --cancel <job-id>` |
| Build (wait) | `cargo build` or `gitforge-build -- cargo build` |
| Build (no wait) | `gitforge-build --no-wait -- cargo build` |
| Bypass wrapper | `cargo-wrapper --wrapper-fallback -- cargo build` |
| Backup config | `./scripts/backup-cargo-wrapper.sh` |
| Uninstall | `./scripts/install-cargo-wrapper.sh --uninstall` |

## Files

| File | Purpose |
|------|---------|
| `scripts/install-cargo-wrapper.sh` | Installer for alias setup |
| `scripts/backup-cargo-wrapper.sh` | Backup/restore utility |
| `crates/gitforce-build/` | Core daemon and CLI |
| `~/.cargo/bin/cargo-wrapper` | Bash wrapper script |
| `/tmp/gitforge-build.sock` | Daemon Unix socket |
