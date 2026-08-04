.PHONY: help install test lint format typecheck build coverage clean test:unit test:integration test:e2e

help:
	@echo "Available targets:"
	@echo "  install        - Install dependencies"
	@echo "  test           - Run all tests (unit + integration)"
	@echo "  test:unit      - Run unit tests only"
	@echo "  test:integration - Run integration tests only"
	@echo "  test:e2e       - Run end-to-end tests only"
	@echo "  lint           - Run ESLint"
	@echo "  format         - Format code with Prettier"
	@echo "  format:check   - Check formatting without modifying"
	@echo "  typecheck      - Run TypeScript type checking"
	@echo "  build          - Compile TypeScript to JavaScript"
	@echo "  coverage       - Run tests with coverage report"
	@echo "  clean          - Remove build artifacts and coverage"

install:
	npm install

test: format:check typecheck
	npm run test

test:unit:
	npm run test:unit

test:integration:
	npm run test:integration

test:e2e:
	npm run test:e2e

lint:
	npm run lint

format:
	npm run format

format:check:
	npm run format:check

typecheck:
	npm run typecheck

build: clean
	npm run build

coverage:
	npm run coverage

clean:
	rm -rf dist coverage
	rm -f .coverage.* coverage.xml
