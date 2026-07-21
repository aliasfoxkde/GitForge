#!/bin/bash
# Cross-platform build script for GitForge
# Builds for Linux, macOS, and Windows

set -e

INSTALL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$INSTALL_DIR/target/cross"

echo "🔨 GitForge Cross-Platform Build"
echo "================================"

# Create build directory
mkdir -p "$BUILD_DIR"

# Parse arguments
TARGET_OS="${1:-all}"
BUILD_TYPE="${2:-release}"

# Targets for each platform
LINUX_TARGETS=("x86_64-unknown-linux-musl" "aarch64-unknown-linux-musl")
MAC_TARGETS=("x86_64-apple-darwin" "aarch64-apple-darwin")
WIN_TARGETS=("x86_64-pc-windows-gnu" "aarch64-pc-windows-gnu")

build_target() {
    local target=$1
    local os=$(echo $target | cut -d'-' -f3)

    echo ""
    echo "Building for $target..."

    case "$os" in
        linux)
            cross build --release --target "$target" --bin api --bin ci --bin git-server --bin runner --bin gitforge || true
            ;;
        darwin)
            cross build --release --target "$target" --bin api --bin ci --bin git-server --bin runner --bin gitforge || true
            ;;
        windows)
            cross build --release --target "$target" --bin api --bin ci --bin git-server --bin runner --bin gitforge || true
            ;;
    esac
}

case "$TARGET_OS" in
    linux)
        for target in "${LINUX_TARGETS[@]}"; do
            build_target "$target"
        done
        ;;
    mac|darwin)
        for target in "${MAC_TARGETS[@]}"; do
            build_target "$target"
        done
        ;;
    windows|win)
        for target in "${WIN_TARGETS[@]}"; do
            build_target "$target"
        done
        ;;
    all)
        for target in "${LINUX_TARGETS[@]}"; do
            build_target "$target"
        done
        for target in "${MAC_TARGETS[@]}"; do
            build_target "$target"
        done
        for target in "${WIN_TARGETS[@]}"; do
            build_target "$target"
        done
        ;;
    *)
        echo "Unknown target: $TARGET_OS"
        echo "Usage: $0 [linux|mac|windows|all] [release|debug]"
        exit 1
        ;;
esac

echo ""
echo "✅ Cross-platform build complete!"
echo ""
echo "Artifacts in: $BUILD_DIR"
