# Makefile for {{PROJECT_NAME}}
# Dark Factory Go template

.PHONY: build test test-race lint vet fmt coverage clean setup

# Binary name
BINARY={{BINARY_NAME}}
VERSION?=0.1.0
BUILD_TIME=$(shell date -u '+%Y-%m-%dT%H:%M:%SZ')

# Go parameters
GOCMD=go
GOBUILD=$(GOCMD) build
GOTEST=$(GOCMD) test
GOVET=$(GOCMD) vet
GOFMT=$(GOCMD) fmt
GOMOD=$(GOCMD) mod

# Directories
SRC_DIR=cmd
BUILD_DIR=bin

# Build the application
build:
	@echo "Building $(BINARY)..."
	@mkdir -p $(BUILD_DIR)
	$(GOBUILD) -o $(BUILD_DIR)/$(BINARY) -ldflags="-s -w -X main.version=$(VERSION) -X main.buildTime=$(BUILD_TIME)" $(SRC_DIR)

# Run tests
test:
	$(GOTEST) -v -p 1 ./...

# Run tests with race detector
test-race:
	$(GOTEST) -v -p 1 -race ./...

# Run tests with coverage
coverage:
	$(GOTEST) -p 1 -coverprofile=coverage.out ./...
	$(GOCMD) tool cover -func=coverage.out

# Format code
fmt:
	$(GOFMT) ./...

# Run go vet
vet:
	$(GOVET) ./...

# Lint with golangci-lint
lint:
	@which golangci-lint > /dev/null || (echo "Installing golangci-lint..." && go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest)
	golangci-lint run --timeout=5m

# Run all quality checks
check: fmt vet lint test

# Download dependencies
deps:
	$(GOMOD) download
	$(GOMOD) tidy

# Clean build artifacts
clean:
	rm -rf $(BUILD_DIR)
	rm -f coverage.out coverage.html

# Setup git hooks
setup:
	@echo "Setting up git hooks..."
	git config core.hooksPath .githooks

# Run the application
run: build
	./$(BUILD_DIR)/$(BINARY)

# Generate a test coverage report
coverhtml: coverage
	$(GOCMD) tool cover -html=coverage.out -o coverage.html

# Build for multiple platforms
build-all:
	@mkdir -p $(BUILD_DIR)
	GOOS=linux GOARCH=amd64 $(GOBUILD) -o $(BUILD_DIR)/$(BINARY)-linux-amd64 $(SRC_DIR)
	GOOS=darwin GOARCH=amd64 $(GOBUILD) -o $(BUILD_DIR)/$(BINARY)-darwin-amd64 $(SRC_DIR)
	GOOS=windows GOARCH=amd64 $(GOBUILD) -o $(BUILD_DIR)/$(BINARY)-windows-amd64.exe $(SRC_DIR)
