# Coverage Guide

How to measure and maintain test coverage in Dark Factory projects.

## Coverage Targets

| Layer | Minimum | Examples |
|-------|--------|----------|
| Core business logic | 95% | `internal/services/*.go` |
| API handlers | 90% | `internal/api/handlers*.go` |
| Configuration | 85% | `internal/config/*.go` |
| Utilities | 85% | `internal/utils/*.go` |

## Measuring Coverage

### Full Coverage Report

```bash
go test -coverprofile=coverage.out ./...
go tool cover -html=coverage.out -o coverage.html
```

### Per-Package Coverage

```bash
go test -coverprofile=coverage.out ./internal/api/...
go tool cover -func=coverage.out
```

### Coverage Summary

```bash
go test -cover ./... -covermode=count
```

## Coverage Enforcement

### Pre-commit Hook

The pre-commit hook runs a coverage gate:

```bash
COVER_THRESHOLD=70  # Default
go test -coverprofile=.coverage.out ./...
COVER=$(go tool cover -func=.coverage.out | grep total | awk '{print $3}' | tr -d '%')
if (( $(echo "$COVER < $COVER_THRESHOLD" | bc -l) )); then
    echo "Coverage $COVER% is below threshold $COVER_THRESHOLD%"
    exit 1
fi
```

### CI/CD Gate

In `.github/workflows/ci.yml`:

```yaml
- name: Check coverage
  run: |
    go test -coverprofile=coverage.out ./...
    go tool cover -func=coverage.out | grep total
```

## What Counts as Covered

- **Line coverage**: Each line executed at least once
- **Branch coverage**: Each branch (if/else, switch cases) exercised
- **Function coverage**: Each function called

## Exemption Process

To request a coverage exemption:

1. Create an issue with `coverage-exemption` label
2. Explain why the code is difficult to test
3. Propose an alternative testing strategy
4. Get approval from CODEOWNERS

Example exemption comment:

```go
// Exempt from coverage: requires integration with external system
// Tracking: #123
func connectToExternalService() {
    // ...
}
```

## Improving Coverage

### Hard-to-Test Code

- **External dependencies**: Use interfaces for mocking
- **Database operations**: Use repository pattern with mocks
- **Network calls**: Use HTTP handler interfaces
- **Time-dependent logic**: Inject clock interface

```go
// Instead of: time.Now()
// Use:
type Clock interface {
    Now() time.Time
}

// Then mock in tests:
type mockClock struct{}
func (m *mockClock) Now() time.Time {
    return time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)
}
```

### Test Fixtures

Use `_fixtures/` directory for test data:

```
_fixtures/
├── testdata.json
└── sample_input.csv
```
