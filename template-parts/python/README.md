# Python Template Part

Opinionated Python project structure following Dark Factory principles.

## Structure

```
project_name/
├── src/
│   └── project_name/
│       ├── __init__.py
│       ├── app.py              # Application entry point
│       ├── config.py           # Configuration management
│       ├── api/
│       │   ├── __init__.py
│       │   ├── routes.py       # FastAPI/Starlette routes
│       │   ├── middleware.py   # Middleware (CORS, auth, logging)
│       │   └── dependencies.py  # FastAPI dependencies
│       ├── models/
│       │   ├── __init__.py
│       │   └── schemas.py      # Pydantic/dataclass models
│       ├── services/
│       │   ├── __init__.py
│       │   └── business.py     # Business logic layer
│       ├── db/
│       │   ├── __init__.py
│       │   ├── connection.py   # Database connection
│       │   └── migrations/     # Alembic migrations
│       └── utils/
│           ├── __init__.py
│           └── logging.py       # Structured logging setup
├── tests/
│   ├── __init__.py
│   ├── conftest.py            # Pytest fixtures
│   ├── unit/
│   │   ├── __init__.py
│   │   └── test_business.py   # Unit tests
│   ├── integration/
│   │   ├── __init__.py
│   │   └── test_api.py        # Integration tests
│   └── e2e/
│       ├── __init__.py
│       └── test_flows.py       # E2E tests
├── scripts/
│   └── run-e2e.sh             # E2E test runner
├── docs/
│   └── API.md                 # API documentation
├── .python-version            # pyenv version file
├── pyproject.toml            # Project metadata + tool configs
├── uv.lock                   # Locked dependencies
├── Makefile                  # Python-specific make targets
└── .ruff.toml                # Ruff linter config
```

## Conventions

1. **uv for all package management** — Never pip directly
2. **Pydantic for models** — All config and data models use Pydantic
3. **Structlog for logging** — Structured JSON logging, not print()
4. **Sentinel errors** — Define `class ProjectNameError(Exception): pass` in each module
5. **Context propagation** — All service functions accept `context.Context`
6. **Type hints everywhere** — No `Any` unless unavoidable
7. **pytest with fixtures** — `conftest.py` for all shared fixtures
8. **Build tags** — `//go:build integration` equivalent: `# pytest.mark.integration`

## Required Files

- `pyproject.toml` with all tool configs (ruff, mypy, pytest, etc.)
- `Makefile` with: `install`, `test`, `lint`, `format`, `typecheck`, `coverage`, `run`
- `.ruff.toml` with strict settings
- `Makefile` with: build, test, lint, coverage, vuln, setup, clean

## Coverage Targets

| Layer | Target |
|-------|--------|
| Core business logic | 95% |
| API handlers | 90% |
| Services | 90% |
| Configuration | 85% |
| Utilities | 85% |
