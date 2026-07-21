# macOS Build & Release Plan

**Version**: 1.0.0
**Last Updated**: 2026-07-21
**Status**: PLANNED

---

## Overview

GitForge supports cross-platform builds for Linux, Windows, and macOS. While Linux and Windows binaries can be built via cross-compilation from any platform, **macOS requires a macOS build environment** due to Apple SDK licensing restrictions.

This document outlines the strategy for building and releasing macOS binaries using a **self-hosted Mac Mini**.

---

## Build Strategy

### Platform Comparison

| Platform | Build Method | Native | Cross-Compile | Notes |
|----------|--------------|--------|---------------|-------|
| Linux | Native or Cross | ✅ | ✅ | Static musl builds recommended |
| Windows | Cross | ❌ | ✅ | Using mingw-w64 |
| macOS | Native Only | ✅ | ❌ | Apple SDK cannot be redistributed |

### Current Support

| Target | Architecture | Status | Build Method |
|--------|--------------|--------|--------------|
| `x86_64-unknown-linux-musl` | x86_64 | ✅ Ready | Native or cross |
| `aarch64-unknown-linux-musl` | ARM64 | ✅ Ready | Native or cross |
| `x86_64-pc-windows-gnu` | x86_64 | ✅ Ready | Cross (mingw) |
| `aarch64-pc-windows-gnu` | ARM64 | ✅ Ready | Cross (mingw) |
| `x86_64-apple-darwin` | x86_64 | 🔲 Planned | Native macOS |
| `aarch64-apple-darwin` | ARM64 | 🔲 Planned | Native macOS |

---

## Self-Hosted Mac Mini Setup

### Hardware Requirements

| Component | Minimum | Recommended |
|-----------|---------|-------------|
| Apple Silicon (M-series) | M1 | M2 Pro or M3 |
| RAM | 8 GB | 16 GB |
| Storage | 256 GB SSD | 512 GB SSD |
| Network | 1 Gbps | 10 Gbps (for fast artifact transfer) |

### Why Mac Mini?

- **Cost-effective**: Starting at $599
- **Low power**: ~6W idle, ~30W under load
- **Quiet**: Fanless design (M1/M2 Mac mini)
- **Self-hosted**: Full control over build environment
- **ARM64 native**: Produces optimized Apple Silicon binaries

### Setup Steps

#### 1. Initial System Configuration

```bash
# Update macOS
sudo softwareupdate -i -a

# Install Xcode Command Line Tools
xcode-select --install

# Install Homebrew (optional but recommended)
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# Install Rust via rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Add macOS targets
rustup target add x86_64-apple-darwin aarch64-apple-darwin
```

#### 2. Install Dependencies

```bash
# Install required tools
brew install git go docker

# For container builds (if using Docker)
brew install --cask docker

# Clone GitForge repository
git clone https://github.com/your-org/gitforge.git
cd gitforge
```

#### 3. Build macOS Binaries

```bash
# Build for both architectures
cargo build --release --target x86_64-apple-darwin --bin gitforge
cargo build --release --target aarch64-apple-darwin --bin gitforge

# Universal binary (fat binary with both architectures)
lipo -create \
  target/x86_64-apple-darwin/release/gitforge \
  target/aarch64-apple-darwin/release/gitforge \
  -output target/gitforge-darwin-universal

# Verify
lipo -info target/gitforge-darwin-universal
# Output: Architectures in the fat file: gitforge are: x86_64 arm64
```

#### 4. Create Release Artifacts

```bash
# Create distribution directory
mkdir -p dist/macos
cd dist/macos

# Create tarball for each architecture
tar -czvf gitforge-x86_64-apple-darwin.tar.gz \
  ../target/x86_64-apple-darwin/release/gitforge

tar -czvf gitforge-aarch64-apple-darwin.tar.gz \
  ../target/aarch64-apple-darwin/release/gitforge

# Create universal binary tarball
tar -czvf gitforge-universal-apple-darwin.tar.gz \
  ../target/gitforge-darwin-universal
```

---

## CI/CD Integration

### GitHub Actions Workflow (macOS Runner)

For teams using GitHub Actions, create a dedicated workflow:

```yaml
# .github/workflows/release-macos.yml
name: Release macOS

on:
  release:
    types: [created]
  workflow_dispatch:

jobs:
  build-macos:
    runs-on: macos-latest
    strategy:
      matrix:
        target: [x86_64-apple-darwin, aarch64-apple-darwin]
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Add targets
        run: rustup target add ${{ matrix.target }}

      - name: Build
        run: cargo build --release --target ${{ matrix.target }} --workspace

      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: gitforge-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/gitforge

  universal:
    needs: build-macos
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4

      - name: Download artifacts
        uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Create universal binary
        run: |
          lipo -create \
            artifacts/gitforge-x86_64-apple-darwin/gitforge \
            artifacts/gitforge-aarch64-apple-darwin/gitforge \
            -output gitforge-universal

      - name: Upload release
        uses: softprops/action-gh-release@v1
        if: github.event_name == 'release'
        with:
          files: |
            gitforge-universal
            artifacts/*/gitforge
```

### Self-Hosted Runner Setup

For the Mac Mini as a self-hosted runner:

```yaml
# .github/workflows/self-hosted-macos.yml
name: Build macOS (Self-Hosted)

on:
  push:
    branches: [main, release/*]
  release:
    types: [created]

jobs:
  build:
    runs-on: [self-hosted, macos, ARM64]
    steps:
      - uses: actions/checkout@v4

      - name: Build
        run: |
          cargo build --release --workspace
          cargo test --release --workspace

      - name: Run coverage
        run: |
          cargo install cargo-llvm-cov
          cargo llvm-cov --release --html
          cargo llvm-cov --release --lcov --output-path lcov.info

      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: gitforge-macos-artifacts
          path: target/release/gitforge

      - name: Upload coverage
        uses: codecov/codecov-action@v4
        with:
          files: lcov.info
```

---

## Installation & Deployment

### macOS Installation

```bash
# Download the latest release
curl -LO https://github.com/your-org/gitforge/releases/latest/download/gitforge-aarch64-apple-darwin.tar.gz

# Extract
tar -xzvf gitforge-aarch64-apple-darwin.tar.gz

# Install to /usr/local/bin
sudo mv gitforge /usr/local/bin/

# Verify
gitforge --version
```

### Homebrew Installation (Future)

```bash
# Once we have a Homebrew tap
brew install gitforge/tap/gitforge
```

### Service Configuration

```bash
# Create plist for launchd
cat > ~/Library/LaunchAgents/com.gitforge.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.gitforge</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/gitforge</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>WorkingDirectory</key>
    <string>/opt/gitforge</string>
</dict>
</plist>
EOF

# Load the service
launchctl load ~/Library/LaunchAgents/com.gitforge.plist
```

---

## Troubleshooting

### Common Issues

#### 1. Code Signing and Notarization

For distribution outside the Mac App Store:

```bash
# Required for macOS 10.15+
# Sign the binary
codesign --sign "Developer ID Application: Your Name" target/release/gitforge

# Notarize (requires Apple Developer account)
xcrun notarytool submit gitforge --apple-id your@email.com --team-id TEAMID --password @keychain:notary
```

#### 2. ARM64/x86_64 Universal Binaries

```bash
# Install using lipo
lipo -create \
  target/x86_64-apple-darwin/release/gitforge \
  target/aarch64-apple-darwin/release/gitforge \
  -output target/gitforge-darwin-universal

# Verify architecture
lipo -info target/gitforge-darwin-universal
```

#### 3. Rust Target Issues

```bash
# If target not found
rustup target add x86_64-apple-darwin aarch64-apple-darwin

# If compilation fails with linker errors
brew install gcc
export CC=gcc-13
export CXX=g++-13
cargo build --release --target aarch64-apple-darwin
```

---

## Release Checklist

- [ ] Build for `x86_64-apple-darwin`
- [ ] Build for `aarch64-apple-darwin`
- [ ] Create universal binary (optional)
- [ ] Sign binary (for distribution)
- [ ] Notarize binary (for macOS 10.15+)
- [ ] Create release artifacts (`.tar.gz`)
- [ ] Upload to GitHub Releases
- [ ] Update Homebrew tap (if applicable)
- [ ] Update installation documentation

---

## Alternative: GitHub Actions macOS Runner

If self-hosting a Mac Mini is not feasible, use GitHub's macOS runners:

```yaml
jobs:
  build-macos:
    runs-on: macos-14  # M1 runner
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: |
          cargo build --release --target aarch64-apple-darwin --workspace
          cargo test --target aarch64-apple-darwin --workspace
```

**Note**: GitHub's macOS runners have usage limits (2000 min/month for free tier).

---

## See Also

- [Cross.toml](../Cross.toml) - Cross-compilation configuration
- [scripts/cross-build.sh](../scripts/cross-build.sh) - Build script
- [PLAN.md](./PLAN.md) - Overall project plan
- [DEPLOYMENT.md](./DEPLOYMENT.md) - Deployment strategies
