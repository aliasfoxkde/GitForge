# Testing Strategy

Dark Factory's layered testing approach ensures comprehensive coverage while maintaining fast feedback loops.

## Testing Layers

### 1. Unit Tests (`tests/unit/`)
- **Purpose**: Test individual functions/methods in isolation
- **Framework**: Go: `testing`, Python: `pytest`, TypeScript: `Vitest`
- **Speed**: < 1ms per test
- **Coverage Target**: 95% for business logic

### 2. Integration Tests (`tests/integration/`)
- **Purpose**: Test component interactions with real infrastructure
- **Build Tag**: `//go:build integration` / `pytest.mark.integration`
- **Speed**: < 1s per test
- **Coverage Target**: 90%

### 3. E2E Tests (`tests/e2e/`)
- **Purpose**: Validate complete user workflows
- **Framework**: Playwright (TypeScript), standard Go tests
- **Speed**: < 30s per test suite
- **Browser Coverage**: Chromium, Firefox, WebKit

## Test Discovery

```bash
# Run all unit tests
go test -p 1 ./...

# Run integration tests only
go test -p 1 -tags=integration ./...

# Run E2E tests
./scripts/run-e2e.sh
```

## Coverage Requirements

| Layer | Target |
|-------|--------|
| Core business logic | 95% |
| API handlers | 90% |
| Configuration | 85% |
| Utilities | 85% |

## CI/CD Enforcement

- Pre-commit: Unit tests + coverage gate (70%)
- Pre-push: Full test suite + coverage (70%)
- CI: All layers with coverage reporting
