#!/bin/bash
# Cross-platform build script for GitForge
# Builds for Linux, macOS, and Windows
#
# IMPORTANT: macOS cross-compilation from Linux is NOT supported due to
# Apple SDK licensing restrictions. Options:
#   - Use GitHub Actions with macos-latest runner
#   - Use a dedicated macOS build machine
#   - Build on macOS hardware directly

set -e

INSTALL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$INSTALL_DIR/target/cross"

echo "🔨 GitForge Cross-Platform Build"
echo "================================"

# Create build directory
mkdir -p "$BUILD_DIR"

# Detect current platform
CURRENT_OS=$(uname -s | tr '[:upper:]' '[:lower:]')

# Parse arguments
TARGET_OS="${1:-all}"
BUILD_TYPE="${2:-release}"
BUILD_FLAGS=()
case "$BUILD_TYPE" in
    release) BUILD_FLAGS+=(--release) ;;
    debug) ;;
    *) echo "Unsupported build type: $BUILD_TYPE (use release or debug)" >&2; exit 2 ;;
esac

# Targets for each platform
LINUX_TARGETS=("x86_64-unknown-linux-musl" "aarch64-unknown-linux-musl")
MAC_TARGETS=("x86_64-apple-darwin" "aarch64-apple-darwin")
WIN_TARGETS=("x86_64-pc-windows-gnu" "aarch64-pc-windows-gnu")

# Services to build
BINARIES=("api" "ci" "git-server" "runner" "gitforge")
BINARIES_ARGS=()
for binary in "${BINARIES[@]}"; do
    BINARIES_ARGS+=(--bin "$binary")
done

build_target() {
    local target=$1
    local os
    os=$(printf '%s\n' "$target" | cut -d'-' -f3)

    echo ""
    echo "Building for $target..."

    case "$os" in
        linux)
            cross build "${BUILD_FLAGS[@]}" --target "$target" "${BINARIES_ARGS[@]}" 2>&1
            ;;
        darwin)
            # macOS cross-compilation from Linux not supported
            echo "⚠️  Skipping $target - macOS cross-compilation requires macOS build environment"
            echo "   Use GitHub Actions macos-latest runner or build on macOS hardware"
            ;;
        windows)
            cross build "${BUILD_FLAGS[@]}" --target "$target" "${BINARIES_ARGS[@]}" 2>&1
            ;;
    esac
}

# Native macOS build (only works on macOS)
build_macos_native() {
    echo ""
    echo "Building for macOS (native)..."
    for target in "${MAC_TARGETS[@]}"; do
        echo "Building for $target..."
        cargo build "${BUILD_FLAGS[@]}" --target "$target" "${BINARIES_ARGS[@]}" 2>&1
    done
}

case "$TARGET_OS" in
    linux)
        for target in "${LINUX_TARGETS[@]}"; do
            build_target "$target"
        done
        ;;
    mac|darwin)
        if [ "$CURRENT_OS" = "darwin" ]; then
            build_macos_native
        else
            echo ""
            echo "⚠️  macOS builds require macOS environment"
            echo "   Options to get macOS binaries:"
            echo "   1. Run this script on a macOS machine"
            echo "   2. Use GitHub Actions with macos-latest runner"
            echo "   3. Set up a dedicated macOS build machine"
            echo ""
            echo "   For CI/CD, push to a branch and use the macOS build runner."
        fi
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
        # macOS requires special handling
        if [ "$CURRENT_OS" = "darwin" ]; then
            build_macos_native
        else
            echo ""
            echo "⚠️  Skipping macOS targets (requires macOS build environment)"
        fi
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
