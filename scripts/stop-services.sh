#!/bin/bash
# GitForge Stop Script

echo "Stopping GitForge services..."

pkill -f "target/release/api" 2>/dev/null && echo "API stopped" || echo "API not running"
pkill -f "target/release/ci" 2>/dev/null && echo "CI stopped" || echo "CI not running"
pkill -f "target/release/git-server" 2>/dev/null && echo "Git Server stopped" || echo "Git Server not running"
pkill -f "target/release/runner" 2>/dev/null && echo "Runner stopped" || echo "Runner not running"

rm -f /nas/Temp/repos/GitForge/data/*.pid

echo "All services stopped"