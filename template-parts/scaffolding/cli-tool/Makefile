# Makefile for CLI Tool
# Supports cross-compilation for multiple platforms

# Variables
BINARY_NAME := cli-tool
VERSION ?= dev
COMMIT ?= $(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")
DATE ?= $(shell date -u +"%Y-%m-%dT%H:%M:%SZ")
BUILDBY ?= make

# Go parameters
GOCMD := go
GOBUILD := $(GOCMD) build
GOTEST := $(GOCMD) test
GOMOD := $(GOCMD) mod
GOFMT := $(GOCMD) fmt
GOVET := $(GOCMD) vet

# Build flags
LD_FLAGS := -s -w \
	-X github.com/example/cli-tool/cmd.version=$(VERSION) \
	-X github.com/example/cli-tool/cmd.commit=$(COMMIT) \
	-X github.com/example/cli-tool/cmd.date=$(DATE) \
	-X github.com/example/cli-tool/cmd.builtBy=$(BUILDBY)

# Platform definitions
PLATFORMS := linux/amd64 linux/arm64 darwin/amd64 darwin/arm64 windows/amd64
BUILD_DIR := dist

# Docker
DOCKER_IMAGE := cli-tool
DOCKER_TAG := $(VERSION)
DOCKER_BUILD := docker build --platform linux/amd64

# Phony targets
.PHONY: all build test lint clean install run docker-build docker-run \
	build-all build-platform help deps fmt vet check cross-build

# Default target
all: deps fmt vet test build

## build: Build the binary for current platform
build:
	@echo "Building $(BINARY_NAME)..."
	@mkdir -p $(BUILD_DIR)
	CGO_ENABLED=0 $(GOBUILD) -ldflags="$(LD_FLAGS)" -o $(BUILD_DIR)/$(BINARY_NAME) ./src/main.go
	@echo "Build complete: $(BUILD_DIR)/$(BINARY_NAME)"

## test: Run tests
test:
	@echo "Running tests..."
	$(GOTEST) -v -race -cover ./...

## lint: Run linter (requires golangci-lint)
lint:
	@echo "Running linter..."
	@if command -v golangci-lint >/dev/null 2>&1; then \
		golangci-lint run ./...; \
	else \
		echo "golangci-lint not installed, skipping..."; \
	fi

## fmt: Format code
fmt:
	@echo "Formatting code..."
	$(GOFMT) ./...

## vet: Run go vet
vet:
	@echo "Running go vet..."
	$(GOVET) ./...

## check: Run all checks (fmt, vet, lint, test)
check: fmt vet lint test

## clean: Remove build artifacts
clean:
	@echo "Cleaning..."
	@rm -rf $(BUILD_DIR)
	@rm -f $(BINARY_NAME)
	@echo "Clean complete"

## install: Install binary to ~/go/bin
install: build
	@echo "Installing..."
	@mkdir -p $(HOME)/go/bin
	@cp $(BUILD_DIR)/$(BINARY_NAME) $(HOME)/go/bin/
	@echo "Installed to $(HOME)/go/bin/$(BINARY_NAME)"

## run: Run the CLI tool
run: build
	@echo "Running..."
	@$(BUILD_DIR)/$(BINARY_NAME) $(ARGS)

## deps: Download dependencies
deps:
	@echo "Downloading dependencies..."
	$(GOMOD) download
	$(GOMOD) tidy

## docker-build: Build Docker image
docker-build:
	@echo "Building Docker image..."
	docker build \
		--platform linux/amd64 \
		--tag $(DOCKER_IMAGE):$(DOCKER_TAG) \
		--build-arg VERSION=$(VERSION) \
		--build-arg COMMIT=$(COMMIT) \
		--build-arg DATE=$(DATE) \
		--build-arg BUILDBY=$(BUILDBY) \
		.

## docker-run: Run CLI in Docker container
docker-run:
	@echo "Running in Docker..."
	docker run --rm --platform linux/amd64 \
		-v $(PWD)/config:/app/config:ro \
		$(DOCKER_IMAGE):$(DOCKER_TAG) $(ARGS)

## build-all: Build for all platforms
build-all: cross-build

## cross-build: Cross-compile for multiple platforms
cross-build:
	@echo "Cross-compiling for all platforms..."
	@mkdir -p $(BUILD_DIR)
	@for platform in $(PLATFORMS); do \
		os=$(word 1,$(subst /, ,$$platform)); \
		arch=$(word 2,$(subst /, ,$$platform)); \
		output_name="$(BINARY_NAME)"; \
		if [ "$$os" = "windows" ]; then output_name="$(BINARY_NAME).exe"; fi; \
		echo "  Building for $$os/$$arch..."; \
		CGO_ENABLED=0 GOOS=$$os GOARCH=$$arch $(GOBUILD) \
			-ldflags="$(LD_FLAGS)" \
			-o $(BUILD_DIR)/$$os-$$arch/$(output_name) \
			./src/main.go; \
	done
	@echo "Cross-build complete:"
	@ls -la $(BUILD_DIR)/*/$(BINARY_NAME)* 2>/dev/null || ls -la $(BUILD_DIR)/*/ 2>/dev/null || echo "  Check $(BUILD_DIR)/"

## build-platform: Build for a specific platform (e.g., make build-platform PLATFORM=linux/arm64)
build-platform:
	@os=$(shell echo $(PLATFORM) | cut -d/ -f1); \
	arch=$(shell echo $(PLATFORM) | cut -d/ -f2); \
	output_name="$(BINARY_NAME)"; \
	if [ "$$os" = "windows" ]; then output_name="$(BINARY_NAME).exe"; fi; \
	echo "Building for $$os/$$arch..."; \
	mkdir -p $(BUILD_DIR)/$$os-$$arch; \
	CGO_ENABLED=0 GOOS=$$os GOARCH=$$arch $(GOBUILD) \
		-ldflags="$(LD_FLAGS)" \
		-o $(BUILD_DIR)/$$os-$$arch/$$output_name \
		./src/main.go

## help: Show this help
help:
	@echo "CLI Tool Makefile"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "  %-20s %s\n", $$1, $$2}' $(MAKEFILE_LIST)
