# Atheon-Enhanced Template

Security pattern scanner for secrets, AI-generated code, accessibility, and web vulnerabilities.

## Features

- **384+ patterns** across 28 categories
- **Secret detection** — API keys, passwords, tokens, PII
- **AI detection** — Identifies AI-generated code shortcuts
- **Quality enforcement** — Detects `git --force`, test skipping
- **MCP server** — AI assistant integration for real-time scanning
- **Streaming API** — Memory-efficient large file scanning
- **SARIF output** — GitHub Security tab integration

## Integration Options

### Option 1: GitHub Actions (Recommended)

Add `.github/workflows/atheon.yml` to your repository:

```yaml
name: Atheon Security Scan

on:
  push:
    branches: [main, stable]
  pull_request:

jobs:
  atheon-scan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Atheon-Enhanced
        run: |
          wget -q https://github.com/aliasfoxkde/Atheon-Enhanced/releases/latest/download/atheon-linux-amd64
          chmod +x atheon-linux-amd64
          sudo mv atheon-linux-amd64 /usr/local/bin/atheon

      - name: Run Atheon Security Scan
        run: |
          atheon --categories=secrets,pii,security --sarif results.sarif ./

      - name: Upload SARIF results
        uses: github/codeql-action/upload-sarif@v4
        with:
          sarif_file: results.sarif
```

### Option 2: Pre-commit Hook

Install the pre-commit hook:

```bash
# Copy hook to your repository
cp template-parts/atheon-enhanced/hooks/pre-commit .git/hooks/
chmod +x .git/hooks/pre-commit

# Or use the installer
./template-parts/atheon-enhanced/hooks/install-atheon-hook.sh
```

The hook scans staged files for secrets and PII before each commit.

### Option 3: MCP Server

For AI assistant integration, run the MCP server:

```bash
# Install
wget -q https://github.com/aliasfoxkde/Atheon-Enhanced/releases/latest/download/atheon-mcp
chmod +x atheon-mcp
sudo mv atheon-mcp /usr/local/bin/

# Add to Claude Code settings
# .claude/settings.json:
{
  "mcpServers": {
    "atheon": {
      "command": "atheon-mcp"
    }
  }
}
```

## Categories

| Category | Description | Patterns |
|----------|-------------|----------|
| `secrets` | API keys, tokens, passwords | 50+ |
| `pii` | Personal identifiable information | 15+ |
| `security` | Web security vulnerabilities | 40+ |
| `ai-detection` | AI-generated code patterns | 25+ |
| `quality` | Code quality anti-patterns | 30+ |
| `compliance` | GDPR, HIPAA, PCI patterns | 5+ |
| `git-hygiene` | Merge conflicts, fixup commits | 5+ |
| `frameworks` | Django, React, Vue, Angular | 25+ |
| `kubernetes` | K8s secrets, configmaps | 10+ |
| `terraform` | Terraform security patterns | 15+ |

Full list: [Pattern Categories](https://github.com/aliasfoxkde/Atheon-Enhanced/blob/main/docs/architecture/PATTERN_CATEGORIES.md)

## Quick Start

```bash
# Scan current directory
atheon --categories=secrets,pii .

# Scan specific file
atheon secrets.py

# CI/CD with SARIF output
atheon --categories=secrets,pii,security --sarif results.sarif ./

# List all patterns
atheon list

# List patterns in category
atheon list --category secrets
```

## Configuration

### Category Selection for Pre-commit

For pre-commit hooks, use only fast, high-confidence patterns:

```bash
atheon --categories=secrets,pii
```

### CI/CD Full Scan

For CI/CD, enable all security categories:

```bash
atheon --categories=secrets,pii,security,ai-detection,quality
```

## Output Formats

### Text (default)
```
secret-api-key  config.py:42
secret-password  user.rb:15
pii-email        users.csv:23
```

### JSON
```bash
atheon --json secrets .
```

### SARIF (GitHub Security)
```bash
atheon --sarif results.sarif --categories=secrets,pii .
```

## Backend Integration

For Rust/Python backends, integrate via:

1. **CLI Subprocess**: Spawn `atheon scan --json` and parse output
2. **MCP Server**: JSON-RPC over stdio
3. **Library**: Future Go library binding (CGO or pure Go)

See [Backend Integration Guide](https://github.com/aliasfoxkde/Atheon-Enhanced/blob/main/docs/guides/BACKEND_INTEGRATION.md)

## Template Files

```
template-parts/atheon-enhanced/
├── README.md                      # This file
├── .github/
│   └── workflows/
│       └── atheon.yml            # GitHub Actions workflow
├── hooks/
│   ├── pre-commit                # Pre-commit hook script
│   └── install-atheon-hook.sh    # Hook installer
└── docs/
    └── INTEGRATION.md            # Detailed integration guide
```

## See Also

- [Atheon-Enhanced Repository](https://github.com/aliasfoxkde/Atheon-Enhanced)
- [Pattern Categories](https://github.com/aliasfoxkde/Atheon-Enhanced/blob/main/docs/architecture/PATTERN_CATEGORIES.md)
- [MCP Integration](https://github.com/aliasfoxkde/Atheon-Enhanced/blob/main/docs/integrations/Mcp.md)
- [Pre-commit Integration](https://github.com/aliasfoxkde/Atheon-Enhanced/blob/main/docs/integrations/Pre-commit.md)
- [MIT License](https://github.com/aliasfoxkde/Atheon-Enhanced/blob/main/LICENSE)
