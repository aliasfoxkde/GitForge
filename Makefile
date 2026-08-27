# ─────────────────────────────────────────────────────────────────────────────
# GitForge Makefile
# Self-hosted Git platform with CI/CD
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: build build-release build-cross test test-race lint clean setup coverage fmt vet

# ─── Build ────────────────────────────────────────────────────────────────────
build:
	cargo build

build-release:
	cargo build --release

# Cross-platform builds
build-cross-linux:
	./scripts/cross-build.sh linux

build-cross-mac:
	./scripts/cross-build.sh mac

build-cross-windows:
	./scripts/cross-build.sh windows

build-cross-all:
	./scripts/cross-build.sh all

# ─── Test ─────────────────────────────────────────────────────────────────────
TEST_TIMEOUT := 15m
COVER_PROFILE := coverage.out

test:
	cargo test

test-release:
	cargo test --release

test-race:
	RUST_BACKTRACE=1 cargo test --release -- --test-threads=1

# ─── Coverage ─────────────────────────────────────────────────────────────────
coverage: test
	@echo "Running coverage..."
	cargo llvm-cov report --all --codecov --output-path=codecov.json || true
	cargo llvm-cov report --all --html --open || true

# ─── Lint ─────────────────────────────────────────────────────────────────────
lint: fmt vet clippy

fmt:
	cargo fmt --check

vet:
	cargo vet

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

# ─── Clean ─────────────────────────────────────────────────────────────────────
clean:
	cargo clean
	rm -rf target/

# ─── Setup ─────────────────────────────────────────────────────────────────────
setup:
	./scripts/install-hooks.sh || true
	@echo "✅ Setup complete."

# ─── Run Services ─────────────────────────────────────────────────────────────
run-api:
	DATABASE_URL="sqlite:///nas/Temp/repos/GitForge/data/gitforge.db?mode=rwc" \
	RUST_LOG=info \
	./target/release/api

run-ci:
	RUST_LOG=info \
	./target/release/ci

run-git-server:
	GIT_ROOT="/nas/Temp/repos/GitForge/repos" \
	RUST_LOG=info \
	./target/release/git-server

run-all: build-release
	./scripts/setup-services.sh

# ─── Stop Services ────────────────────────────────────────────────────────────
stop:
	./scripts/stop-services.sh

# ─── Docker ───────────────────────────────────────────────────────────────────
docker-build:
	docker build -t gitforge:latest .

docker-up:
	docker-compose up -d

docker-down:
	docker-compose down

# ─── Release ──────────────────────────────────────────────────────────────────
release:
	goreleaser release --clean

release-snapshot:
	goreleaser release --clean --snapshot

# ─── Help ─────────────────────────────────────────────────────────────────────
help:
	@echo "GitForge Makefile Commands:"
	@echo ""
	@echo "  make build              - Build debug version"
	@echo "  make build-release     - Build release version"
	@echo "  make build-cross-all   - Build for Linux, macOS, Windows"
	@echo ""
	@echo "  make test              - Run tests"
	@echo "  make test-release      - Run tests in release mode"
	@echo "  make test-race         - Run tests with race detector"
	@echo ""
	@echo "  make lint              - Run linters"
	@echo "  make fmt               - Check formatting"
	@echo ""
	@echo "  make run-all           - Build and run all services"
	@echo "  make stop              - Stop all services"
	@echo ""
	@echo "  make docker-build      - Build Docker image"
	@echo "  make docker-up         - Start with Docker Compose"
	@echo "  make docker-down       - Stop Docker Compose"
	@echo ""
	@echo "  make release           - Create release with GoReleaser"
