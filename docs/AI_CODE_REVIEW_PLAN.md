# AI Code Review Feature - Implementation Plan

## Overview

Implement a local AI-powered code review system into GitForge, inspired by Pullfrog's approach. The system will analyze code changes, provide inline review comments, suggest fixes, and integrate with existing CI/CD pipelines - all running locally to reduce costs by using the user's own API keys.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      GitForge AI Code Review                      │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐ │
│  │ Git Protocol  │  │ Change       │  │ AI Provider          │ │
│  │ Integration   │──│ Parser       │──│ Interface           │ │
│  │ (Git2)       │  │ (Diff/MR)    │  │ (Anthropic/OpenAI)  │ │
│  └──────────────┘  └──────────────┘  └──────────────────────┘ │
│         │                  │                    │              │
│         ▼                  ▼                    ▼              │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              Review Engine (Core Logic)                  │  │
│  │  - Diff analysis    - Context gathering                 │  │
│  │  - Pattern detection - Cost management                  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                              ▼                                   │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              Output Formatters                            │  │
│  │  - Inline comments  - Summary reports  - CI annotations   │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Key Components

### 1. AI Provider Interface (`gitforce-ai` crate)
- **Supported Providers**: Anthropic (Claude), OpenAI, Local OLLAMA
- **BYOK Model**: Uses user's existing API keys
- **Cost Controls**: Per-review budget, batch optimization

### 2. Change Parser (`gitforce-review` crate)
- **Diff Extraction**: Parse unified diffs from git
- **MR/PR Support**: Integrate with GitLab MRs / GitHub PRs
- **Context Window**: Optimize prompts for context limits

### 3. Review Engine
- **Pattern Detection**: Common bugs, security issues, style violations
- **Code Analysis**: Complexity, duplication, dead code
- **Security Scanning**: Vulnerability detection

### 4. Output Formatters
- **Inline Comments**: GitHub/GitLab comment format
- **CLI Summary**: Human-readable review summary
- **CI Annotations**: GitHub Actions annotations format

## Implementation Phases

---

### Phase 1: Core Infrastructure

| Task | Description |
|------|-------------|
| 1.1 | Create `gitforce-ai` crate with provider traits |
| 1.2 | Implement Anthropic provider (Claude 3.5 Sonnet) |
| 1.3 | Implement OpenAI provider fallback |
| 1.4 | Create `gitforce-review` crate |
| 1.5 | Build diff parser for git changes |

**Deliverable**: Basic AI review of staged changes via CLI

---

### Phase 2: Enhanced Review Capabilities

| Task | Description |
|------|-------------|
| 2.1 | Implement multi-file context analysis |
| 2.2 | Add security vulnerability scanning |
| 2.3 | Implement code pattern detection |
| 2.4 | Add complexity and quality scoring |
| 2.5 | Build review caching for cost optimization |

**Deliverable**: Comprehensive review with security and quality analysis

---

### Phase 3: CI/CD Integration

| Task | Description |
|------|-------------|
| 3.1 | GitHub Actions integration |
| 3.2 | GitLab CI integration |
| 3.3 | Build inline comment posting |
| 3.4 | Implement review status reporting |

**Deliverable**: Automated PR reviews in CI pipelines

---

### Phase 4: Advanced Features

| Task | Description |
|------|-------------|
| 4.1 | Implement fix suggestion engine |
| 4.2 | Add auto-fix PR creation |
| 4.3 | Build review dashboard (web UI) |
| 4.4 | Implement review history/trends |

**Deliverable**: End-to-end review workflow with fix automation

---

## Usage Examples

### CLI Review
```bash
# Review staged changes
gitforge review --staged

# Review specific files
gitforge review --diff HEAD~1

# Review with high context
gitforge review --context full

# CI mode (automated)
gitforge review --ci --pr-url https://github.com/user/repo/pull/123
```

### Configuration
```yaml
# .gitforge.yaml
ai:
  provider: anthropic  # or openai, ollama
  model: claude-3-5-sonnet-20241022
  api_key_env: ANTHROPIC_API_KEY
  
review:
  max_files: 50
  max_cost_per_review: $0.50
  security_scan: true
  pattern_checks: true
  
output:
  format: github  # or gitlab, cli, json
```

## Cost Optimization

1. **Context Batching**: Group related files to reduce API calls
2. **Caching**: Cache review results for unchanged files
3. **Smart Diffing**: Only review changed portions
4. **Budget Limits**: Hard cap on per-review spend
5. **Model Selection**: Use cheaper models for simple reviews

## Reference: Pullfrog Insights

Pullfrog architecture to draw from:
- MCP (Model Context Protocol) for context management
- Trigger-based activation (@mentions, PR events)
- Prompt templating with Zod schemas
- Model-agnostic BYOK approach

## Dependencies

- Existing GitForge infrastructure
- `git2` crate for git operations
- `reqwest` for API calls
- `async-trait` for async interfaces
- `serde` for configuration

## Testing Strategy

1. **Unit Tests**: Parser, provider mocks, formatters
2. **Integration Tests**: Real API calls with test keys
3. **E2E Tests**: Full review workflow with fixtures
4. **Cost Tests**: Verify budget limits work

## Timeline Estimate

- Phase 1: 1-2 weeks
- Phase 2: 1-2 weeks  
- Phase 3: 1 week
- Phase 4: 2-3 weeks

Total: 5-8 weeks for full implementation
