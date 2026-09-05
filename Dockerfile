# GitForge Dockerfile
# Multi-stage build for minimal production image

# =============================================================================
# Build stage
# =============================================================================
FROM rust:1.80-bookworm as builder

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
COPY crates/gitforge-runner ./crates/gitforge-runner
COPY crates/gitforge-sandbox ./crates/gitforge-sandbox
COPY crates/gitforge-scheduler ./crates/gitforge-scheduler
COPY crates/gitforge-storage ./crates/gitforge-storage
COPY crates/gitforge-api ./crates/gitforge-api
COPY services/api ./services/api
COPY services/ci ./services/ci
COPY services/runner ./services/runner
COPY services/git-server ./services/git-server

# Build all binaries
RUN cargo build --release --bin api --bin ci --bin runner --bin git-server

# =============================================================================
# Runner build stage (separate because it needs Docker)
# =============================================================================
FROM builder as runner-builder

# Build runner
RUN cargo build --release --bin runner

# =============================================================================
# Production stage - API server
# =============================================================================
FROM debian:bookworm-slim as api-prod

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

# Set ownership
RUN chown -R gitforge:gitforge /app

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
FROM debian:bookworm-slim as ci-prod

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash gitforge

# Copy binary
COPY --from=builder /app/target/release/ci /app/ci

# Set ownership
RUN chown -R gitforge:gitforge /app

USER gitforge

EXPOSE 42781

ENTRYPOINT ["/app/ci"]

# =============================================================================
# Production stage - Runner
# =============================================================================
FROM debian:bookworm-slim as runner-prod

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

# Set ownership
RUN chown -R gitforge:gitforge /app

USER gitforge

ENTRYPOINT ["/app/runner"]

# =============================================================================
# Production stage - Git server
# =============================================================================
FROM debian:bookworm-slim as git-server-prod

WORKDIR /app

# Install runtime dependencies and OpenSSH
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    openssh-server \
    git \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash gitforge

# Copy binary
COPY --from=builder /app/target/release/git-server /app/git-server

# Setup SSH directory
RUN mkdir -p /home/gitforge/.ssh && chmod 700 /home/gitforge/.ssh

# Set ownership
RUN chown -R gitforge:gitforge /home/gitforge

USER gitforge

EXPOSE 42022 42782

ENTRYPOINT ["/app/git-server"]
