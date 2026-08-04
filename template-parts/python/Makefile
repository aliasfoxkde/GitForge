# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Python Makefile
# Requires: uv, python 3.11+
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: install test lint format typecheck coverage vuln clean run

PYTEST_ARGS := -v --strict-markers --tb=short
COVERAGE_MIN := 70

# ─── Install ──────────────────────────────────────────────────────────────────
install:
	uv sync --frozen-lockfile

install-dev:
	uv sync --frozen-lockfile --extra dev

# ─── Test ─────────────────────────────────────────────────────────────────────
test:
	uv run pytest $(PYTEST_ARGS) tests/

test-unit:
	uv run pytest $(PYTEST_ARGS) -m unit tests/

test-integration:
	uv run pytest $(PYTEST_ARGS) -m integration tests/

test-e2e:
	uv run pytest $(PYTEST_ARGS) -m e2e tests/

# ─── Format ───────────────────────────────────────────────────────────────────
format:
	uv run ruff check --fix .
	uv run ruff format .

# ─── Lint ─────────────────────────────────────────────────────────────────────
lint:
	uv run ruff check . --output-format=text

# ─── Typecheck ────────────────────────────────────────────────────────────────
typecheck:
	uv run mypy src/ --no-error-summary

# ─── Coverage ─────────────────────────────────────────────────────────────────
coverage:
	uv run pytest $(PYTEST_ARGS) --cov=src --cov-report=term-missing --cov-fail-under=$(COVERAGE_MIN) tests/

coverage-html:
	uv run pytest $(PYTEST_ARGS) --cov=src --cov-report=html:coverage-html tests/

# ─── Vulnerability ─────────────────────────────────────────────────────────────
vuln:
	uv pip audit || true

# ─── Run ──────────────────────────────────────────────────────────────────────
run:
	uv run uvicorn src.project_name.app:app --reload

# ─── Clean ────────────────────────────────────────────────────────────────────
clean:
	rm -rf .pytest_cache .mypy_cache .ruff_cache coverage.xml coverage-html/
	find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
	find . -type f -name "*.pyc" -delete 2>/dev/null || true
