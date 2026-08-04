# syntax=docker/dockerfile:1

# ─────────────────────────────────────────────────────────────────────────────
# Stage 1: Build
# ─────────────────────────────────────────────────────────────────────────────
FROM --platform=$BUILDPLATFORM golang:1.24-alpine AS builder

# Install build tools
RUN apk add --no-cache git ca-certificates tzdata

WORKDIR /src

# Copy go mod files and download dependencies first (layer cache optimization)
COPY go.mod go.sum ./
RUN go mod download

# Copy source
COPY . .

# Build for the target platform
ARG TARGETOS
ARG TARGETARCH
ARG VERSION=dev
ARG COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")

RUN CGO_ENABLED=0 GOOS=${TARGETOS} GOARCH=${TARGETARCH} \
    go build -ldflags="\
      -s -w \
      -X main.version=${VERSION} \
      -X main.commit=${COMMIT} \
    " \
    -o /worker-service \
    ./src/main.go

# ─────────────────────────────────────────────────────────────────────────────
# Stage 2: Runtime
# ─────────────────────────────────────────────────────────────────────────────
FROM alpine:3.21 AS runtime

LABEL org.opencontainers.image.title="Worker Service"
LABEL org.opencontainers.image.description="Background job processing worker"
LABEL org.opencontainers.image.source="https://github.com/yourorg/worker-service"
LABEL org.opencontainers.image.version="1.0.0"
LABEL org.opencontainers.image.licenses="MIT"
LABEL maintainer="yourorg@example.com"

# Install runtime dependencies
RUN apk add --no-cache ca-certificates tzdata dumb-init

# Create non-root user
RUN addgroup -g 1000 appgroup && \
    adduser -u 1000 -G appgroup -s /bin/sh -D appuser

WORKDIR /app

# Copy binary from builder
COPY --from=builder /worker-service /app/worker-service

# Copy application config (if any)
COPY --from=builder /src/src/config/ /app/config/

# Change ownership
RUN chown -R appuser:appgroup /app

USER appuser

# Use dumb-init to handle PID 1 responsibilities (signal handling, zombie reaping)
ENTRYPOINT ["dumb-init", "--"]

# Run as non-root user
CMD ["./worker-service"]

# Expose metrics port
EXPOSE 9090

# Hardening: no new privileges, seccomp profile
SECURITYCMD ["--no-new-privs"]
