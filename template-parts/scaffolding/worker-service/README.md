# Worker Service

A production-ready background job processing service written in Go. It connects to RabbitMQ (or Kafka), processes jobs concurrently with configurable worker pools, and exposes Prometheus metrics.

## Features

- **Multi-worker pool**: Configurable number of concurrent workers via `WORKER_COUNT`
- **Graceful shutdown**: Drains in-flight jobs before exiting
- **Structured logging**: JSON logs via `log/slog`
- **Prometheus metrics**: Exposed on port 9090 (configurable)
- **Queue abstraction**: RabbitMQ implementation with interface for Kafka swap-in
- **Docker-ready**: Multi-stage build, non-root user, healthchecks
- **Configuration via env vars**: No config files required

## Architecture

```
src/
├── main.go           # Entry point, signal handling, worker orchestration
├── worker/
│   ├── processor.go  # Job processing business logic
│   └── queue.go      # RabbitMQ consumer
└── config/
    └── config.go     # Environment variable configuration
```

## Prerequisites

- Go 1.24+
- RabbitMQ 3.x (or Kafka)
- PostgreSQL 17+ (optional, for job persistence)
- Docker & Docker Compose (for containerized run)

## Quick Start

### 1. Clone and build

```bash
git clone https://github.com/yourorg/worker-service.git
cd worker-service
make deps
make build
```

### 2. Start dependencies

```bash
docker-compose up -d
```

Wait for RabbitMQ and PostgreSQL to become healthy:

```bash
docker-compose ps
```

### 3. Run

```bash
./worker-service
```

Or use Docker directly:

```bash
make docker-run
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `AMQP_URL` | `amqp://guest:guest@localhost:5672/` | RabbitMQ connection URL. Takes precedence over Kafka. |
| `KAFKA_BROKERS` | `` | Comma-separated Kafka broker list (used when `AMQP_URL` is empty) |
| `WORKER_COUNT` | `4` | Number of concurrent worker goroutines |
| `LOG_LEVEL` | `info` | Log level: `debug`, `info`, `warn`, `error` |
| `SHUTDOWN_TIMEOUT` | `30` | Max seconds to wait for graceful shutdown |
| `DATABASE_DSN` | `` | PostgreSQL connection string (optional) |
| `METRICS_ADDR` | `:9090` | Address for Prometheus metrics endpoint |

## Queue Message Format

Jobs are JSON messages published to the `jobs` queue:

```json
{
  "id": "uuid-v4",
  "type": "example",
  "payload": {
    "message": "hello world"
  },
  "retries": 0,
  "max_retries": 3
}
```

### Job Types

| Type | Description |
|---|---|
| `example` | Example job type (demonstrates processing) |
| Any other value | Acknowledged and discarded (dead-letter safe) |

### Adding Custom Job Types

Edit `src/worker/processor.go` and extend the `ProcessMessage` switch:

```go
case "my-job":
    result = p.handleMyJob(ctx, job)
```

## API / Metrics

Prometheus metrics are exposed on `:9090/metrics`.

| Metric | Type | Description |
|---|---|---|
| `worker_jobs_processed_total` | Counter | Total jobs processed by type and status |
| `worker_job_duration_seconds` | Histogram | Job processing duration |
| `worker_queue_depth` | Gauge | Current queue depth (RabbitMQ) |

## Building

### Local binary

```bash
make build
```

### Docker image

```bash
make docker-build VERSION=1.0.0
```

## Testing

```bash
make test
```

With race detection and coverage:

```bash
make test
```

## Development

```bash
make vet        # Static analysis
make fmtcheck   # Format check
make lint       # Full lint (requires golangci-lint)
```

## Docker Compose Services

| Service | Port | Description |
|---|---|---|
| `postgres` | `5432` | PostgreSQL 17 |
| `rabbitmq` | `5672` | RabbitMQ AMQP |
| `rabbitmq` | `15672` | RabbitMQ Management UI (guest/guest) |
| `worker` | `9090` | Prometheus metrics |

## Graceful Shutdown

On `SIGINT` or `SIGTERM`:

1. Stop accepting new jobs
2. Wait for in-flight jobs to complete (up to `SHUTDOWN_TIMEOUT` seconds)
3. Close queue connections
4. Exit cleanly

## License

MIT
