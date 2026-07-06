# Go Template Part

Opinionated Go project structure following Dark Factory principles.

## Structure

```
cmd/app/              # Application entry point
internal/app/         # Application core (not importable externally)
internal/config/      # Configuration
internal/logger/      # Structured logging (log/slog)
internal/api/         # HTTP/API layer
internal/db/          # Database layer
pkg/                  # Public packages (importable by others)
module/
  ├── go.mod
  ├── Makefile
  └── .golangci.yml
```

## Conventions

1. **Sentinel errors** — Define `var ErrXxx = errors.New("...")` in each package
2. **Context propagation** — All public APIs accept `context.Context`
3. **No global state** — Use `App` struct with dependencies
4. **Graceful shutdown** — Signal handling with configurable timeout
5. **Structured logging** — `log/slog` only, no `fmt.Fprintf`
6. **RE2 regex only** — No PCRE
7. **`-p 1`** — Mandatory for `go test` (package-level init state)
8. **`go mod tidy`** — Run before every commit

## Required Files

- `go.mod` with `go 1.21` minimum
- `.golangci.yml` with strict settings
- `Makefile` with: `build`, `test`, `lint`, `vuln`, `setup`, `clean`
- `codecov.yml` with 90% project threshold
