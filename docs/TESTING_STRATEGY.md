# Testing Strategy

## Philosophy

> **Build coverage as you go — not at the end.**

Problems compound when testing is deferred. The Dark Factory approach:
1. Write the failing test first
2. Write the code to make it pass
3. Document WHY (not just WHAT)
4. Repeat — coverage grows incrementally

## Coverage Targets

| Layer | Target | Mandatory |
|-------|--------|-----------|
| Core business logic | 95% | ✅ |
| API handlers | 90% | ✅ |
| Configuration | 85% | ✅ |
| Utilities | 85% | ✅ |
| E2E tests | 80% | Optional |

## Test Types

### Unit Tests

- Fast (< 1ms each)
- Isolated — test one function/method
- No I/O, no network, no database
- Mock external dependencies

```go
// Unit test in Go
func TestFindPattern_MatchesJWT(t *testing.T) {
    scanner := NewScanner(patterns)
    results := scanner.Scan("Bearer eyJ...")
    require.Len(t, results, 1)
    require.Equal(t, "jwt-token", results[0].Pattern.Name)
}
```

```python
# Unit test in Python
def test_validate_email_returns_true_for_valid_email():
    result = validate_email("alice@example.com")
    assert result is True
```

### Integration Tests

- Verify components work together
- Real database (test instance)
- Real HTTP client (test server)
- Marked with build tag: `//go:build integration` or `pytest.mark.integration`

### E2E Tests

- Full application flow
- Real everything
- Marked with: `//go:build e2e` or `pytest.mark.e2e`

## Coverage Enforcement

| Check | Gate | Where |
|-------|------|-------|
| Unit test coverage | 70%+ | CI (required) |
| Core logic coverage | 95% | CI (required) |
| E2E coverage | 80% | CI (optional) |

## How to Write Tests

### Test Naming

```
test_<unit>_<scenario>_<expected_result>
```

| Language | Example |
|----------|---------|
| Go | `TestFindPattern_MatchesJWT_TokenDetected` |
| Python | `test_find_pattern_matches_jwt_token_detected` |

### Test Structure (Arrange-Act-Assert)

```go
// Arrange
scanner := NewScanner(testPatterns)

// Act
results := scanner.Scan("Bearer token")

// Assert
require.Len(t, results, 1)
require.Equal(t, "jwt-token", results[0].Pattern.Name)
```

### Test Isolation

Each test must be independent:
- No shared mutable state
- Setup/teardown for resources
- Fresh database for each test

## Running Tests

```bash
# All tests
make test

# Unit tests only
go test -p 1 -v ./...

# With race detector
make test-race

# Coverage report
make coverage
make coverage-html

# Python
make test-unit
make test-integration
make test-e2e
```

## CI Integration

Tests run on every push and PR. The pipeline:

1. **lint** — `golangci-lint`, `ruff` — must pass
2. **test** — all Go + Python tests — must pass
3. **coverage** — must meet threshold — blocks merge if below
4. **build** — cross-platform — must succeed

## Adding a New Test

1. Find the appropriate test file (`*_test.go` or `test_*.py`)
2. If it doesn't exist, create it in the same package
3. Write the test with `func TestXxx...` (Go) or `def test_xxx...` (Python)
4. Run: `go test -p 1 -v ./... -run TestXxx` (Go) or `pytest -v -k TestXxx` (Python)
5. Verify it fails (if writing a new feature test)
6. Implement the feature
7. Verify test passes
8. Verify coverage