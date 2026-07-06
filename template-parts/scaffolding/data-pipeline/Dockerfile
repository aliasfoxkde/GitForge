# Build stage
FROM docker.io/golang:1.23-alpine AS builder

# Install build dependencies
RUN apk add --no-cache git ca-certificates tzdata

WORKDIR /build

# Copy go mod files
COPY go.mod go.sum* ./

# Download dependencies
RUN go mod download

# Copy source code
COPY . .

# Build binary with security flags
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 \
    go build -ldflags="-extldflags '-static' -w -s" \
    -o etl-pipeline .

# Runtime stage
FROM docker.io/alpine:3.19

# Install runtime dependencies
RUN apk add --no-cache ca-certificates tzdata && \
    addgroup -g 1000 etl && \
    adduser -u 1000 -G etl -s /bin/sh -D etl

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/etl-pipeline .

# Create data directory
RUN mkdir -p /data && chown etl:etl /data

# Set labels
LABEL org.opencontainers.image.title="ETL Data Pipeline"
LABEL org.opencontainers.image.description="Production-ready ETL data pipeline"
LABEL org.opencontainers.image.version="1.0.0"
LABEL org.opencontainers.image.authors="Engineering Team"
LABEL org.opencontainers.image.source="https://github.com/example/etl-pipeline"

# Switch to non-root user
USER etl:etl

# Expose metrics port
EXPOSE 9090

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:9090/health || exit 1

ENTRYPOINT ["./etl-pipeline"]