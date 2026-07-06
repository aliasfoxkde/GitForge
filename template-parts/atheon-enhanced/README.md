# Atheon-Enhanced Template

Security pattern scanner for secrets, AI-generated code, accessibility, and web vulnerabilities.

## Features

- **274+ patterns** across 19 categories
- **Secret detection** — API keys, passwords, tokens, PII
- **AI detection** — Identifies AI-generated code shortcuts
- **Quality enforcement** — Detects `git --force`, test skipping
- **MCP server** — AI assistant integration for real-time scanning
- **Streaming API** — Memory-efficient large file scanning

## Installation

```bash
git clone https://github.com/aliasfoxkde/Atheon-Enhanced.git
cd Atheon-Enhanced
go build -o atheon ./cmd/atheon
sudo mv atheon /usr/local/bin/
```

## Quick Start

```bash
# Scan current directory
atheon --categories=secrets,pii .

# Use pipeline profile for CI/CD
atheon --profile config/profiles/pipeline.json ./

# Pre-commit hook
atheon --categories=secrets,pii --staged
```

## Pre-commit Integration

Add to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: atheon-secrets
        name: Atheon Secrets Scan
        entry: atheon --categories=secrets,pii --staged
        language: system
        types: [go, python, typescript, bash]
```

## CI/CD Integration

```bash
# GitHub Actions
- name: Run Atheon Security Scan
  run: |
    atheon --categories=secrets,pii,security ./...
```

## Configuration Profiles

| Profile | Use Case |
|---------|----------|
| `config/profiles/pipeline.json` | CI/CD pipelines |
| `config/profiles/development.json` | Local development |
| `config/profiles/mcp.json` | AI assistant integration |

## MCP Server

For AI assistant integration, add to your `.claude/settings.json`:

```json
{
  "mcpServers": {
    "atheon": {
      "command": "atheon",
      "args": ["--mcp"]
    }
  }
}
```

## Categories

- `secrets` — API keys, tokens, passwords
- `pii` — Personal identifiable information
- `security` — Web security vulnerabilities
- `accessibility` — a11y issues
- `ai-detection` — AI-generated code patterns
- `quality` — Code quality anti-patterns

## See Also

- [Atheon-Enhanced Repository](https://github.com/aliasfoxkde/Atheon-Enhanced)
- [MIT License](https://github.com/aliasfoxkde/Atheon-Enhanced/blob/main/LICENSE)
