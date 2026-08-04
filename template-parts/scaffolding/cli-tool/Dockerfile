# Multi-stage build for CLI tool
# Stage 1: Build
FROM golang:1.21-alpine AS builder

# Install build dependencies
RUN apk add --no-cache git ca-certificates tzdata

# Set working directory
WORKDIR /build

# Copy go mod files first for better layer caching
COPY go.mod go.sum ./
RUN go mod download

# Copy source code
COPY . .

# Build flags for version info
ARG VERSION=dev
ARG COMMIT=unknown
ARG DATE=unknown
ARG BUILDBY=goreleaser

# Build the binary with optimizations
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 GO111MODULE=on \
    go build -ldflags=" \
      -s -w \
      -X github.com/example/cli-tool/cmd.version=${VERSION} \
      -X github.com/example/cli-tool/cmd.commit=${COMMIT} \
      -X github.com/example/cli-tool/cmd.date=${DATE} \
      -X github.com/example/cli-tool/cmd.builtBy=${BUILDBY} \
    " \
    -o cli-tool \
    ./src/main.go

# Stage 2: Runtime
FROM alpine:3.19

# Install runtime dependencies
RUN apk add --no-cache ca-certificates tzdata

# Create non-root user for security
RUN addgroup -g 1000 appgroup && \
    adduser -u 1000 -G appgroup -s /bin/sh -D appuser

# Set working directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/cli-tool /app/

# Create config directory
RUN mkdir -p /app/config && chown -R appuser:appgroup /app

# Switch to non-root user
USER appuser

# Set environment variables
ENV HOME=/app
ENV PATH=/app

# Metadata labels
LABEL org.opencontainers.image.title="CLI Tool"
LABEL org.opencontainers.image.description="A production-ready CLI application template"
LABEL org.opencontainers.image.version="1.0.0"
LABEL org.opencontainers.image.source="https://github.com/example/cli-tool"
LABEL org.opencontainers.image.authors="Example Team"
LABEL org.opencontainers.image.licenses="MIT"

# Expose no ports (CLI tool)

# Default command
ENTRYPOINT ["/app/cli-tool"]
CMD ["--help"]
