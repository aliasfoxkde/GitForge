# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Makefile
# ─────────────────────────────────────────────────────────────────────────────

.PHONY: build test test-race lint vuln bundle setup clean coverage fmt vet golangci

# ─── Build ────────────────────────────────────────────────────────────────────
build: build-ateon build-mcp

build-ateon:
	go build -o bin/atheon ./cmd/atheon

build-mcp:
	go build -o bin/atheon-mcp ./cmd/mcp

# ─── Test ─────────────────────────────────────────────────────────────────────
TEST_TIMEOUT := 15m
COVER_PROFILE := coverage.out

test:
	go test -p 1 -timeout $(TEST_TIMEOUT) -coverprofile=$(COVER_PROFILE) ./...

test-race:
	go test -p 1 -race -timeout $(TEST_TIMEOUT) -coverprofile=$(COVER_PROFILE) ./...

# ─── Coverage ─────────────────────────────────────────────────────────────────
coverage: test
	@COV=$$(go tool cover -func=$(COVER_PROFILE) | grep total | awk '{print $$3}' | tr -d '%'); \
	echo "Coverage: $$COV%"; \
	if (( $$(echo "$$COV < 70" | bc -l) )); then \
		echo "ERROR: coverage $$COV% below threshold 70%"; \
		exit 1; \
	fi

coverage-html: test
	go tool cover -html=$(COVER_PROFILE) -o coverage.html

# ─── Lint ─────────────────────────────────────────────────────────────────────
lint: vet fmt golangci goimports

vet:
	go vet ./...

fmt:
	@UNFORMATTED=$$(gofmt -l .); \
	if [ -n "$$UNFORMATTED" ]; then \
		echo "gofmt must format these files:"; \
		echo "$$UNFORMATTED"; \
		exit 1; \
	fi

golangci:
	golangci-lint run --timeout=5m --concurrency=2

goimports:
	@which goimports > /dev/null || go install golang.org/x/tools/cmd/goimports@latest
	@UNIMPORTED=$$(goimports -l .); \
	if [ -n "$$UNIMPORTED" ]; then \
		echo "goimports must format these files:"; \
		echo "$$UNIMPORTED"; \
		exit 1; \
	fi

# ─── Vulnerability ────────────────────────────────────────────────────────────
vuln:
	go install golang.org/x/vuln/cmd/govulncheck@latest
	govulncheck ./...

# ─── Bundle ───────────────────────────────────────────────────────────────────
bundle:
	go run ./bundler

# ─── Setup ─────────────────────────────────────────────────────────────────────
setup:
	./scripts/install-hooks.sh
	@echo "✅ Setup complete. Git hooks installed at .githooks/"

# ─── Clean ─────────────────────────────────────────────────────────────────────
clean:
	rm -rf bin/ coverage.out coverage.html report.xml .aetheon-build/