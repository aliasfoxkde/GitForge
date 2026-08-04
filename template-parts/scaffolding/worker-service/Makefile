.PHONY: build run test docker-build docker-run clean lint vet fmtcheck

# Binary name
BINARY_NAME=worker-service
VERSION?=dev
COMMIT=$(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")

# Go settings
GOCMD=go
GOBUILD=$(GOCMD) build
GOTEST=$(GOCMD) test
GOMOD=github.com/yourorg/worker-service
GOSRC=$(shell find src -name '*.go')

# Docker
DOCKER_IMAGE=yourorg/worker-service
DOCKER_TAG=$(VERSION)

all: build

## build: compile the binary
build:
	@echo "Building $(BINARY_NAME)..."
	@CGO_ENABLED=0 $(GOBUILD) \
		-ldflags="-s -w -X main.version=$(VERSION) -X main.commit=$(COMMIT)" \
		-o $(BINARY_NAME) \
		./src/main.go
	@echo "Built: ./$(BINARY_NAME)"

## run: build and run locally (requires RabbitMQ/Postgres via docker-compose)
run: build
	@echo "Running $(BINARY_NAME) (make sure dependencies are up: docker-compose up -d)..."
	@./$(BINARY_NAME)

## test: run unit tests with race detection
test:
	@echo "Running tests..."
	@$(GOTEST) -v -race -coverprofile=coverage.out ./src/...
	@echo "Coverage report:"
	@$(GOCMD) tool cover -func=coverage.out | tail -1

## lint: run golangci-lint
lint:
	@echo "Linting..."
	@golangci-lint run ./src/...

## vet: run go vet
vet:
	@echo "Running vet..."
	@$(GOCMD) vet ./src/...

## fmtcheck: check formatting
fmtcheck:
	@echo "Checking formatting..."
	@if [ -n "$$(gofmt -l src/)" ]; then \
		echo "Formatting issues found:"; \
		gofmt -l src/; \
		exit 1; \
	fi
	@echo "Formatting OK"

## docker-build: build Docker image
docker-build:
	@echo "Building Docker image $(DOCKER_IMAGE):$(DOCKER_TAG)..."
	@docker build \
		--build-arg VERSION=$(VERSION) \
		--build-arg COMMIT=$(COMMIT) \
		-t $(DOCKER_IMAGE):$(DOCKER_TAG) \
		-t $(DOCKER_IMAGE):latest \
		-f Dockerfile \
		.
	@echo "Built: $(DOCKER_IMAGE):$(DOCKER_TAG)"

## docker-run: run via docker-compose
docker-run:
	@echo "Starting services with docker-compose..."
	@docker-compose up -d
	@echo "Services started. Metrics available at http://localhost:9090"
	@echo "RabbitMQ Management UI: http://localhost:15672 (guest/guest)"

## docker-stop: stop docker-compose services
docker-stop:
	@echo "Stopping services..."
	@docker-compose down

## docker-clean: remove containers, volumes, and images
docker-clean:
	@echo "Cleaning Docker resources..."
	@docker-compose down -v --rmi local 2>/dev/null || true

## clean: remove built artifacts
clean:
	@echo "Cleaning..."
	@rm -f $(BINARY_NAME) coverage.out
	@echo "Cleaned."

## deps: download Go dependencies
deps:
	@echo "Downloading dependencies..."
	@$(GOCMD) mod download
	@$(GOCMD) mod verify

## help: show this help
help:
	@echo "Usage: make [target]"
	@echo ""
	@echo "Targets:"
	@grep -E '^## ' Makefile | sed 's/## /  /'
