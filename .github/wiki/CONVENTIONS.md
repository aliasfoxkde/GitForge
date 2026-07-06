# Conventions

Coding and collaboration conventions for Dark Factory projects.

## Conventional Commits

Format: `<type>(<scope>): <description>`

### Types

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation only |
| `test` | Adding or updating tests |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `ci` | CI/CD changes |
| `chore` | Maintenance tasks |
| `perf` | Performance improvements |

### Examples

```
feat(api): add user authentication endpoint
fix(auth): resolve null pointer in token validation
docs: update README with new installation steps
test(handler): add tests for user creation
refactor(db): extract connection pooling logic
ci: add golangci-lint to pre-commit
chore: update dependencies
perf(query): optimize database index lookup
```

## Go Coding Standards

### Sentinel Errors

Define package-level error variables:

```go
var (
    ErrNotFound     = errors.New("not found")
    ErrInvalidInput = errors.New("invalid input")
    ErrUnauthorized = errors.New("unauthorized")
)
```

### Context Propagation

All public APIs must accept `context.Context`:

```go
func (s *Service) DoSomething(ctx context.Context, req *Request) (*Response, error)
```

### Structured Logging

Use `log/slog` instead of `fmt.Fprintf`:

```go
slog.Info("request processed",
    "request_id", req.ID,
    "duration_ms", elapsed.Milliseconds(),
)
```

### Dependency Injection

Pass dependencies via struct fields, not globals:

```go
type Service struct {
    db     *DB
    logger *slog.Logger
    cache  *Cache
}
```

## Test Requirements

### Coverage by Layer

| Layer | Minimum Coverage |
|-------|-----------------|
| Core business logic | 95% |
| API handlers | 90% |
| Configuration | 85% |
| Utilities | 85% |

### Test Patterns

**Table-driven tests:**

```go
func TestAdd(t *testing.T) {
    tests := []struct {
        name string
        a, b int
        want int
    }{
        {"positive", 1, 2, 3},
        {"negative", -1, -2, -3},
        {"zero", 0, 0, 0},
    }
    for _, tt := range tests {
        t.Run(tt.name, func(t *testing.T) {
            got := Add(tt.a, tt.b)
            if got != tt.want {
                t.Errorf("Add() = %v, want %v", got, tt.want)
            }
        })
    }
}
```

**Use `-p 1`** — mandatory for `go test` due to package-level init state.

## Code Review Checklist

- [ ] Conventional commit format in title
- [ ] Tests written before implementation
- [ ] Coverage maintained or improved
- [ ] No `//golint:disable` without justification
- [ ] No hardcoded credentials or secrets
- [ ] No debug/temporary code
- [ ] `go vet ./...` passes
- [ ] `golangci-lint run --timeout=5m` passes
- [ ] Documentation updated (if needed)
