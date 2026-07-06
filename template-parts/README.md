# Dark Factory Template Parts

Modular, language-specific templates that can be composed to bootstrap new projects.

## Structure

```
template-parts/
├── README.md                          # This file
├── go/
│   ├── README.md                      # Go template part documentation
│   ├── module.go                      # Standard module structure
│   ├── cmd/
│   │   └── app/
│   │       └── main.go               # Standard CLI entry point
│   ├── internal/
│   │   ├── app/
│   │   │   └── app.go               # Application struct + lifecycle
│   │   ├── config/
│   │   │   └── config.go            # Configuration management
│   │   └── logger/
│   │       └── logger.go            # Structured logging setup
│   ├── api/
│   │   ├── server.go                # HTTP server with graceful shutdown
│   │   └── middleware.go            # Standard middleware (CORS, auth, logging)
│   ├── db/
│   │   └── migrations/              # Database migration files
│   ├── _fixtures/
│   │   └── example_test.go         # Example test patterns
│   ├── Makefile                     # Go-specific make targets
│   └── go.mod                       # Module definition
├── e2e-testing/
│   ├── README.md
│   ├── framework/
│   │   ├── harness.go              # Test harness + setup/teardown
│   │   ├── session.go              # Test session management
│   │   ├── reporter.go             # Coverage + results reporter
│   │   ├── ai_coverage.go         # AI-driven coverage analyzer
│   │   └── debugger.go             # Debug utilities for E2E failures
│   ├── tests/
│   │   ├── smoke_test.go          # Basic smoke test template
│   │   ├── integration_test.go     # Full integration test template
│   │   └── regression_test.go      # Regression test template
│   └── scripts/
│       ├── run-e2e.sh             # E2E runner script
│       └── coverage-report.sh      # Coverage aggregation script
├── code-library/
│   ├── README.md
│   ├── snippets/
│   │   ├── go/
│   │   │   ├── error_handling.go   # Sentinel error patterns
│   │   │   ├── context_patterns.go # Context propagation patterns
│   │   │   ├── retry_patterns.go  # Retry with backoff
│   │   │   └── graceful_shutdown.go # Signal handling + drain
│   │   ├── bash/
│   │   │   ├── robust_script.sh    # Error handling, safe defaults
│   │   │   └── api_calls.sh        # HTTP/API call patterns
│   │   └── common/
│   │       ├── git_hooks.sh        # Hook installation helpers
│   │       └── ci_validation.sh    # CI validation patterns
│   └── docs/
│       ├── ARCHITECTURE.md         # Architecture decision records
│       ├── API_CONVENTIONS.md      # API design conventions
│       └── TESTING_STRATEGY.md     # How to write effective tests
└── common/
    ├── CLAUDE.md                    # AI instruction template
    ├── AGENTS.md                    # Agent behavior rules
    └── .github/
        ├── CODEOWNERS              # Parameterized code owners
        ├── PULL_REQUEST_TEMPLATE.md
        ├── ISSUE_TEMPLATE/
        │   ├── bug_report.yml
        │   ├── feature_request.yml
        │   └── config.yml
        ├── dependabot.yml
        └── FUNDING.yml
```

## Usage

```bash
# Copy a template part into a new project
cp -r template-parts/go/cmd/my-app/cmd/
cp -r template-parts/e2e-testing/ my-e2e-tests/
cp -r template-parts/code-library/ ./docs/

# Or use the bootstrap script
./scripts/setup-repo.sh owner new-repo --include-go --include-e2e
```

## Coverage Targets

| Component | Target |
|-----------|--------|
| Core business logic | 95%+ |
| API handlers | 90%+ |
| E2E coverage | 80%+ |
| Configuration | 85%+ |
| Utility/helper functions | 85%+ |

## Stack Coverage Goals

| Stack | Target |
|-------|--------|
| Go | 90%+ |
| Python | 85%+ |
| Bash/Shell | 70%+ (linting coverage) |
| JavaScript/TypeScript | 85%+ |
