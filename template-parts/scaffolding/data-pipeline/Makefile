.PHONY: build run test docker-build docker-run clean lint fmt vendor test-integration help

# Variables
BINARY_NAME=etl-pipeline
BUILD_DIR=./build
DOCKER_IMAGE=etl-pipeline:latest
DOCKER_COMPOSE_FILE=docker-compose.yml

# Go commands
GO=go
GOTEST=$(GO) test
GOBUILD=$(GO) build
GOFMT=gofmt
GOLINT=golangci-lint

# Build the binary
build: fmt lint
	@echo "Building $(BINARY_NAME)..."
	@mkdir -p $(BUILD_DIR)
	CGO_ENABLED=0 $(GOBUILD) -ldflags="-w -s" -o $(BUILD_DIR)/$(BINARY_NAME) ./src
	@echo "Binary built at $(BUILD_DIR)/$(BINARY_NAME)"

# Run locally (requires PostgreSQL and Redis)
run: build
	@echo "Running $(BINARY_NAME)..."
	@./$(BUILD_DIR)/$(BINARY_NAME)

# Run with Docker Compose
docker-run:
	@echo "Starting services with Docker Compose..."
	docker-compose -f $(DOCKER_COMPOSE_FILE) up -d
	@echo "Services started. Metrics available at http://localhost:9090/metrics"

# Stop Docker Compose services
docker-stop:
	@echo "Stopping services..."
	docker-compose -f $(DOCKER_COMPOSE_FILE) down

# Run unit tests
test:
	@echo "Running tests..."
	$(GOTEST) -v -race -coverprofile=coverage.out ./src/...
	@echo "Coverage report:"
	$(GO) tool cover -func=coverage.out

# Run tests with verbose output
test-verbose:
	$(GOTEST) -v -race ./src/...

# Run integration tests (requires docker-compose services)
test-integration:
	@echo "Running integration tests..."
	$(GOTEST) -v -race -tags=integration ./src/...

# Build Docker image
docker-build:
	@echo "Building Docker image $(DOCKER_IMAGE)..."
	docker build -t $(DOCKER_IMAGE) .
	@echo "Image built: $(DOCKER_IMAGE)"

# Build Docker image with no cache (full rebuild)
docker-build-no-cache:
	@echo "Building Docker image $(DOCKER_IMAGE) (no cache)..."
	docker build --no-cache -t $(DOCKER_IMAGE) .

# Push Docker image
docker-push: docker-build
	@echo "Pushing $(DOCKER_IMAGE)..."
	docker push $(DOCKER_IMAGE)

# Format code
fmt:
	@echo "Formatting code..."
	$(GOFMT) -s -w ./src

# Lint code
lint:
	@echo "Linting code..."
	$(GOLINT) run ./src/... || true

# Vendor dependencies
vendor:
	@echo "Vendoring dependencies..."
	$(GO) mod vendor

# Tidy go.mod
tidy:
	@echo "Tidying go.mod..."
	$(GO) mod tidy

# Clean build artifacts
clean:
	@echo "Cleaning..."
	@rm -rf $(BUILD_DIR)
	@rm -f coverage.out
	docker-compose -f $(DOCKER_COMPOSE_FILE) down -v --remove-orphans 2>/dev/null || true
	@echo "Clean complete"

# Full rebuild
rebuild: clean build

# Development mode - run with hot reload (requires air)
dev:
	@which air > /dev/null || (echo "Installing air..." && go install github.com/cosmtrek/air@latest)
	@air

# Generate test coverage report
coverage: test
	$(GO) tool cover -html=coverage.out -o coverage.html
	@echo "Coverage report generated: coverage.html"

# Run docker-compose logs
logs:
	docker-compose -f $(DOCKER_COMPOSE_FILE) logs -f

# Show help
help:
	@echo "ETL Data Pipeline - Makefile targets:"
	@echo ""
	@echo "  build             - Build the binary (format + lint + build)"
	@echo "  run               - Build and run locally"
	@echo "  docker-run        - Start services with docker-compose"
	@echo "  docker-stop       - Stop docker-compose services"
	@echo "  test              - Run unit tests with coverage"
	@echo "  test-verbose      - Run tests with verbose output"
	@echo "  test-integration  - Run integration tests"
	@echo "  docker-build      - Build Docker image"
	@echo "  docker-build-no-cache - Build Docker image without cache"
	@echo "  docker-push       - Push Docker image to registry"
	@echo "  fmt               - Format source code"
	@echo "  lint              - Run linter"
	@echo "  vendor            - Vendor dependencies"
	@echo "  tidy              - Tidy go.mod"
	@echo "  clean             - Clean build artifacts and containers"
	@echo "  rebuild           - Clean and rebuild"
	@echo "  dev               - Run with hot reload (requires air)"
	@echo "  coverage          - Generate HTML coverage report"
	@echo "  logs              - Show docker-compose logs"
	@echo "  help              - Show this help"