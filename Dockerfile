# GitForge Dockerfile
# Multi-stage build for minimal production image

# =============================================================================
# Build stage
# =============================================================================
FROM rust:1.98-bookworm AS builder

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY crates/gitforge-common ./crates/gitforge-common
COPY crates/gitforge-db ./crates/gitforge-db
COPY crates/gitforge-events ./crates/gitforge-events
COPY crates/gitforge-ci ./crates/gitforge-ci
COPY crates/gitforge-core ./crates/gitforge-core
COPY crates/gitforge-process ./crates/gitforge-process
COPY crates/gitforge-build ./crates/gitforge-build
COPY crates/gitforge-runner ./crates/gitforge-runner
COPY crates/gitforge-sandbox ./crates/gitforge-sandbox
COPY crates/gitforge-scheduler ./crates/gitforge-scheduler
COPY crates/gitforge-storage ./crates/gitforge-storage
COPY crates/gitforge-api ./crates/gitforge-api
COPY crates/gitforge-cli ./crates/gitforge-cli
COPY crates/gitforge-ai ./crates/gitforge-ai
COPY crates/gitforge-review ./crates/gitforge-review
COPY services/api ./services/api
COPY services/ci ./services/ci
COPY services/runner ./services/runner
COPY services/git-server ./services/git-server

# Build all binaries
RUN cargo build --locked --release --bin api --bin ci --bin runner --bin git-server

# =============================================================================
# Runner build stage (separate because it needs Docker)
# =============================================================================
FROM builder AS runner-builder

# Build runner
RUN cargo build --locked --release --bin runner

# =============================================================================
# Production stage - API server
# =============================================================================
FROM debian:bookworm-slim AS api-prod

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash gitforge

# Copy binary
COPY --from=builder /app/target/release/api /app/api

# Set ownership. Volume mountpoints must exist and belong to the service
# user before a named volume is first mounted: Docker seeds a fresh volume
# from the image directory, so a root-owned mountpoint makes /data and /git
# read-only for the non-root gateway.
RUN mkdir -p /data /git && chown -R gitforge:gitforge /app /data /git

USER gitforge

# Expose port
EXPOSE 42780

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:42780/health || exit 1

ENTRYPOINT ["/app/api"]

# =============================================================================
# Production stage - CI service
# =============================================================================
FROM debian:bookworm-slim AS ci-prod

WORKDIR /app

# Install runtime dependencies. git is required: the orchestrator reads the
# committed .gitforce.yml and clones run workspaces by shelling out to git.
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    git \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash gitforge

# Copy binary
COPY --from=builder /app/target/release/ci /app/ci

# Set ownership (mountpoint ownership rationale: see api-prod stage)
RUN mkdir -p /data && chown -R gitforge:gitforge /app /data

USER gitforge

EXPOSE 42781

ENTRYPOINT ["/app/ci"]

# =============================================================================
# Production stage - Runner
# =============================================================================
FROM debian:bookworm-slim AS runner-prod

WORKDIR /app

# Install runtime dependencies and Docker CLI
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    docker.io \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash gitforge

# Copy binary
COPY --from=runner-builder /app/target/release/runner /app/runner

# Set ownership (mountpoint ownership rationale: see api-prod stage)
RUN mkdir -p /data && chown -R gitforge:gitforge /app /data

USER gitforge

ENTRYPOINT ["/app/runner"]

# =============================================================================
# Production stage - Git server
# =============================================================================
FROM debian:bookworm-slim AS git-server-prod

WORKDIR /app

# Install runtime dependencies. Git over SSH is served in-process by the
# binary (russh), so there is no sshd in the container; git is required to
# serve upload-pack/receive-pack child processes.
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    git \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash gitforge

# Copy binary
COPY --from=builder /app/target/release/git-server /app/git-server

# Setup SSH directory
RUN mkdir -p /home/gitforge/.ssh && chmod 700 /home/gitforge/.ssh

# Set ownership (mountpoint ownership rationale: see api-prod stage)
RUN mkdir -p /data /git && chown -R gitforge:gitforge /home/gitforge /data /git

USER gitforge

EXPOSE 42022 42782

ENTRYPOINT ["/app/git-server"]
