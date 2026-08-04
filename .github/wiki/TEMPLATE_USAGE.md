# Template Usage

How to use Dark Factory's modular template parts to build your project.

## Template Parts Overview

| Part | Purpose |
|------|---------|
| `go/` | Go module structure with standard packages |
| `python/` | Python project with uv, ruff, pytest |
| `typescript/` | TypeScript project with ESLint, Vitest |
| `rust/` | Rust project with Cargo, clippy |
| `e2e-testing/` | Playwright-based E2E test harness |
| `code-library/` | Reusable code snippets |
| `scaffolding/` | Pre-built project templates |
| `common/` | Shared configs (CLAUDE.md, AGENTS.md, .github/) |
| `atheon-enhanced/` | Security scanning integration |

## Scaffolding Options

Pre-built project templates in `scaffolding/`:

| Template | Description |
|----------|-------------|
| `api-service/` | Go HTTP API with handlers, middleware, config |
| `cli-tool/` | Go CLI with Cobra commands |
| `worker-service/` | Go background worker with queue |
| `data-pipeline/` | Go data processing pipeline |
| `repository-ruleset/` | GitHub ruleset configuration |

## Combining Template Parts

### Building a Go API Service

```bash
# Create project directory
mkdir my-api && cd my-api
git init

# Copy template parts
cp -r template-parts/go/* .
cp -r template-parts/scaffolding/api-service/* .
cp -r template-parts/common/. .
cp -r template-parts/e2e-testing/* .

# Customize
# 1. Replace {{PROJECT_NAME}} and {{GITHUB_OWNER}} placeholders
find . -type f -exec sed -i 's/{{PROJECT_NAME}}/my-api/g' {} \;
find . -type f -exec sed -i 's/{{GITHUB_OWNER}}/your-username/g' {} \;

# Setup git hooks
make setup

# First commit
git add -A && git commit -m "feat: initial project structure"
```

### Building a TypeScript Library

```bash
mkdir my-lib && cd my-lib
git init
cp -r template-parts/typescript/* .
cp -r template-parts/common/. .
# Customize placeholders
make setup
```

## Bootstrap Script

Use `scripts/setup-repo.sh` for automated setup:

```bash
./scripts/setup-repo.sh owner/project --public \
    --include-go \
    --include-e2e \
    --include-atheon
```

### Options

| Option | Description |
|--------|-------------|
| `--include-go` | Include Go template |
| `--include-python` | Include Python template |
| `--include-typescript` | Include TypeScript template |
| `--include-e2e` | Include E2E testing framework |
| `--include-atheon` | Include Atheon security scanner |
| `--include-common` | Include common configs |
| `--private` | Create private repository |
| `--public` | Create public repository |

## Customization Checklist

After copying template parts:

- [ ] Replace `{{PROJECT_NAME}}` with project name
- [ ] Replace `{{GITHUB_OWNER}}` with GitHub username/org
- [ ] Update `LICENSE` if needed
- [ ] Update `README.md` with project-specific docs
- [ ] Configure `.github/CODEOWNERS`
- [ ] Set up secrets in repository settings
- [ ] Verify CI/CD pipeline runs
- [ ] Enable required GitHub Apps (e.g., Codecov)

## Adding Template Parts to Existing Project

```bash
# Add E2E testing to existing project
cp -r template-parts/e2e-testing/ .

# Add common configs
cp -r template-parts/common/.github/ .

# Add Atheon security scanning
cp -r template-parts/atheon-enhanced/ .
```
