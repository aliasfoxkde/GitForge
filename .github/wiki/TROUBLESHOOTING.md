# Troubleshooting

Common issues and how to resolve them.

## Tests Failing

1. Check that you're using the correct Go version: `go version`
2. Run tests with verbose output: `go test -v ./...`
3. Ensure git hooks are installed: `git config core.hooksPath`
4. Try cleaning cache: `go clean -testcache`

## Pre-commit Hook Failures

1. Check git hooks path: `git config core.hooksPath`
2. Re-install hooks: `make setup`
3. Skip hooks temporarily: `git commit --no-verify` (use sparingly)

## Coverage Below Threshold

1. Run coverage report: `go test -coverprofile=coverage.out ./...`
2. View uncovered code: `go tool cover -html=coverage.out`
3. Add tests for uncovered functions
4. Update threshold in `vars.COVERAGE_THRESHOLD` if genuinely untestable

## Build Failures

1. Update dependencies: `go mod tidy`
2. Clear build cache: `go clean -cache`
3. Check Go version: `go version` (minimum: 1.21)

## Still Stuck?

- Open an issue with the `bug` label
- Include: Go version, OS, full error output, steps to reproduce
