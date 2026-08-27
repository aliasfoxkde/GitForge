#!/bin/bash
# GitForge Complete Setup Script
# Sets up directories, environment, and runs all services

set -e

INSTALL_DIR="/nas/Temp/repos/GitForge"
DATA_DIR="$INSTALL_DIR/data"
REPOS_DIR="$INSTALL_DIR/repos"
ARTIFACTS_DIR="$INSTALL_DIR/artifacts"
CACHE_DIR="$INSTALL_DIR/cache"
LOGS_DIR="$INSTALL_DIR/logs"
SSH_DIR="$INSTALL_DIR/ssh"

echo "🔧 GitForge Setup Script"
echo "========================"

# Create required directories
echo "Creating directories..."
mkdir -p "$DATA_DIR" "$REPOS_DIR" "$ARTIFACTS_DIR" "$CACHE_DIR" "$LOGS_DIR" "$SSH_DIR"
chmod 777 "$DATA_DIR" "$REPOS_DIR" "$ARTIFACTS_DIR" "$CACHE_DIR" "$LOGS_DIR" "$SSH_DIR"

# Build if needed
if [ ! -f "$INSTALL_DIR/target/release/api" ]; then
    echo "Building GitForge..."
    cd "$INSTALL_DIR"
    cargo build --release
fi

# Kill any existing services
echo "Stopping existing services..."
pkill -f "target/release/api" 2>/dev/null || true
pkill -f "target/release/ci" 2>/dev/null || true
pkill -f "target/release/git-server" 2>/dev/null || true
pkill -f "target/release/runner" 2>/dev/null || true
sleep 2

# Start API service
echo "Starting API service..."
cd "$INSTALL_DIR"
DATABASE_URL="sqlite:///nas/Temp/repos/GitForge/data/gitforge.db?mode=rwc" \
RUST_LOG=info \
./target/release/api > "$LOGS_DIR/api.log" 2>&1 &
API_PID=$!
echo "API started with PID $API_PID"

sleep 3

# Verify API is running
if curl -s http://localhost:8080/health > /dev/null 2>&1; then
    echo "✅ API is healthy"
else
    echo "❌ API failed to start - check $LOGS_DIR/api.log"
    cat "$LOGS_DIR/api.log"
    exit 1
fi

# Start CI service
echo "Starting CI service..."
cd "$INSTALL_DIR"
RUST_LOG=info \
./target/release/ci > "$LOGS_DIR/ci.log" 2>&1 &
CI_PID=$!
echo "CI started with PID $CI_PID"

# Start Git Server
echo "Starting Git Server..."
cd "$INSTALL_DIR"
GIT_ROOT="$REPOS_DIR" \
RUST_LOG=info \
./target/release/git-server > "$LOGS_DIR/git-server.log" 2>&1 &
GIT_PID=$!
echo "Git Server started with PID $GIT_PID"

echo ""
echo "✅ GitForge Services Started!"
echo ""
echo "Services:"
echo "  API:        http://localhost:8080 (PID $API_PID)"
echo "  CI:         PID $CI_PID"
echo "  Git Server: PID $GIT_PID"
echo ""
echo "Endpoints:"
echo "  Health:      http://localhost:8080/health"
echo "  Swagger UI:  http://localhost:8080/swagger-ui"
echo "  Metrics:     http://localhost:8080/metrics"
echo "  OpenAPI:     http://localhost:8080/api-docs/openapi.json"
echo ""
echo "Logs:"
echo "  $LOGS_DIR/api.log"
echo "  $LOGS_DIR/ci.log"
echo "  $LOGS_DIR/git-server.log"
echo ""

# Save PIDs for later stop
echo "$API_PID" > "$DATA_DIR/api.pid"
echo "$CI_PID" > "$DATA_DIR/ci.pid"
echo "$GIT_PID" > "$DATA_DIR/git.pid"

echo "Run './scripts/stop-services.sh' to stop all services"
