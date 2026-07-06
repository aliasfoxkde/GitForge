.PHONY: build run test docker-build docker-run clean lint fmt

# Go parameters
GOCMD=go
GOBUILD=$(GOCMD) build
GOTEST=$(GOCMD) test
GOMOD=github.com/example/api-service
BINARY_NAME=api-service
BUILD_DIR=./bin

# Docker
DOCKER_IMAGE=api-service
DOCKER_TAG=latest

# Build the binary
build:
	@echo "Building..."
	@mkdir -p $(BUILD_DIR)
	$(GOBUILD) -ldflags="-w -s" -trimpath -o $(BUILD_DIR)/$(BINARY_NAME) ./src

# Run the service locally
run: build
	@echo "Running..."
	$(BUILD_DIR)/$(BINARY_NAME)

# Run tests
test:
	@echo "Running tests..."
	$(GOTEST) -v -race -cover ./src/...

# Run tests with coverage report
test-coverage:
	@echo "Running tests with coverage..."
	$(GOTEST) -v -race -coverprofile=coverage.out ./src/...
	$(GOCMD) tool cover -html=coverage.out -o coverage.html

# Format code
fmt:
	@echo "Formatting..."
	gofmt -s -w ./src

# Lint code
lint:
	@echo "Linting..."
	golangci-lint run ./src/... || go vet ./src/...

# Docker build
docker-build:
	@echo "Building Docker image..."
	docker build -t $(DOCKER_IMAGE):$(DOCKER_TAG) .

# Docker run with docker-compose
docker-run:
	@echo "Starting services with docker-compose..."
	docker-compose up -d
	@echo "API service running at http://localhost:8080"

# Stop Docker services
docker-stop:
	@echo "Stopping services..."
	docker-compose down

# Remove Docker volumes
docker-clean:
	@echo "Removing Docker volumes..."
	docker-compose down -v

# Clean build artifacts
clean:
	@echo "Cleaning..."
	rm -rf $(BUILD_DIR)
	rm -f coverage.out coverage.html

# Full clean: binaries + docker
dist-clean: clean docker-clean

# Help
help:
	@echo "Available targets:"
	@echo "  build         - Build the binary"
	@echo "  run           - Build and run locally"
	@echo "  test          - Run tests"
	@echo "  test-coverage - Run tests with coverage report"
	@echo "  fmt           - Format code"
	@echo "  lint          - Lint code"
	@echo "  docker-build  - Build Docker image"
	@echo "  docker-run    - Start services with docker-compose"
	@echo "  docker-stop   - Stop Docker services"
	@echo "  docker-clean  - Remove Docker volumes"
	@echo "  clean         - Remove build artifacts"
	@echo "  dist-clean    - Full clean (build + docker)"