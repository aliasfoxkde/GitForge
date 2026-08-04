# Dark Factory Code Library

Reusable snippets, patterns, and documentation for rapid development.

## Philosophy

> **Build test, code, and documentation coverage as you go — not all at once at the end.**

Problems compound when you defer testing and documentation. The Dark Factory approach:
1. Write a failing test first
2. Write the code to make it pass
3. Document WHY (not just WHAT)
4. Repeat — coverage grows incrementally

## Snippet Categories

### Go Patterns
- `snippets/go/error_handling.go` — Sentinel errors, error wrapping, error chains
- `snippets/go/context_patterns.go` — Context propagation, cancellation, timeouts
- `snippets/go/retry_patterns.go` — Retry with exponential backoff, jitter
- `snippets/go/graceful_shutdown.go` — Signal handling, drain, timeout

### Bash Patterns
- `snippets/bash/robust_script.sh` — `set -euo pipefail`, error handling, logging
- `snippets/bash/api_calls.sh` — curl wrappers, retry, rate limiting

### Common Patterns
- `snippets/common/git_hooks.sh` — Hook installation, validation
- `snippets/common/ci_validation.sh` — CI environment detection, validation

## Coverage Targets

When using these snippets, maintain these coverage targets:

| Category | Target | Why |
|----------|--------|-----|
| Error paths | 95%+ | Error handling is where bugs hide |
| Edge cases | 90%+ | Null, empty, boundary conditions |
| Configuration | 85%+ | Env vars, flags, defaults |
| Critical paths | 100% | Core business logic never drops below 95% |

## Usage

Copy snippets into your project. Each snippet is self-contained and tested.

```bash
# Copy a specific snippet
cp template-parts/code-library/snippets/go/error_handling.go ./internal/errors/

# Or import the whole library as a submodule
git submodule add https://github.com/aliasfoxkde/dark-factory.git code-library
```

## Contributing

When adding a new snippet:
1. Write the snippet with inline tests (`_test.go`)
2. Document the WHY in the doc comment
3. Include usage examples
4. Add to this README in the appropriate category
