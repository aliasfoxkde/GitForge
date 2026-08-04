# ETL Data Pipeline

Production-ready ETL (Extract, Transform, Load) data pipeline built with Go, PostgreSQL, Redis, and Prometheus monitoring.

## Architecture

```
┌─────────────┐    ┌──────────────┐    ┌─────────────┐
│   Source    │───▶│  Transform   │───▶│ Destination │
│  (Extract)  │    │   Workers    │    │   (Load)    │
└─────────────┘    └──────────────┘    └─────────────┘
       │                  │                   │
       └──────────────────┴───────────────────┘
                         │
              ┌──────────┴──────────┐
              │   Redis (Queue)     │
              │  Prometheus (Metrics)│
              └─────────────────────┘
```

## Features

- **Multi-stage processing**: Extract, Transform, Load with parallel workers
- **Multiple source types**: CSV, HTTP API, PostgreSQL
- **Multiple destinations**: PostgreSQL, stdout (JSON lines)
- **Batch processing**: Configurable batch sizes for efficiency
- **Graceful shutdown**: Handles SIGINT/SIGTERM properly
- **Metrics**: Prometheus metrics for monitoring
- **Health checks**: HTTP health and readiness endpoints
- **Docker support**: Multi-stage Dockerfile, docker-compose for full stack

## Quick Start

### Prerequisites

- Go 1.21+
- Docker and Docker Compose
- PostgreSQL 16+ (if running locally)
- Redis 7+ (if running locally)

### Local Development

1. **Clone and setup**

```bash
git clone https://github.com/example/etl-pipeline.git
cd etl-pipeline
```

2. **Create sample data**

```bash
mkdir -p data
cat > data/source.csv << 'EOF'
id,name,amount,category
1,Product A,19.99,electronics
2,Product B,29.99,clothing
3,Product C,9.99,books
EOF
```

3. **Run with Docker Compose**

```bash
docker-compose up -d
docker-compose logs -f etl-pipeline
```

4. **Or run locally**

```bash
# Start dependencies
docker-compose up -d postgres redis prometheus

# Run the pipeline
make run
```

## Configuration

All configuration is via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `SOURCE_TYPE` | `csv` | Source type: `csv`, `http`, `postgres` |
| `SOURCE_PATH` | `/data/source.csv` | Path to CSV file (for csv source) |
| `SOURCE_URL` | `http://api.example.com/data` | URL for HTTP source |
| `SOURCE_INTERVAL` | `1m` | Interval between fetches (for http source) |
| `DEST_TYPE` | `postgres` | Destination type: `postgres`, `stdout` |
| `DEST_CONN` | `` | Destination connection string |
| `REDIS_ADDR` | `localhost:6379` | Redis address |
| `REDIS_PASSWORD` | `` | Redis password |
| `REDIS_DB` | `0` | Redis database number |
| `DB_HOST` | `localhost` | PostgreSQL host |
| `DB_PORT` | `5432` | PostgreSQL port |
| `DB_USER` | `etl` | PostgreSQL user |
| `DB_PASSWORD` | `etlsecret` | PostgreSQL password |
| `DB_NAME` | `etl_db` | PostgreSQL database name |
| `DB_POOL_SIZE` | `10` | PostgreSQL connection pool size |
| `BATCH_SIZE` | `100` | Records per batch |
| `WORKERS` | `4` | Number of transform workers |
| `SHUTDOWN_TIMEOUT` | `30s` | Graceful shutdown timeout |
| `METRICS_PORT` | `9090` | Metrics HTTP server port |

## Usage

### Makefile Commands

```bash
make build         # Build the binary
make run           # Build and run locally
make test          # Run unit tests with coverage
make docker-build  # Build Docker image
make docker-run    # Start with docker-compose
make clean         # Clean build artifacts
make logs          # View docker-compose logs
```

### Docker Compose Services

The stack includes:

- **etl-pipeline**: The main ETL pipeline
- **postgres**: PostgreSQL 16 for data storage
- **redis**: Redis 7 for queueing/caching
- **prometheus**: Prometheus for metrics collection

### Monitoring

**Metrics endpoint**: `http://localhost:9090/metrics`

Key metrics:
- `etl_records_processed_total{stage, status}` - Total records processed
- `etl_processing_duration_seconds{stage}` - Processing time histogram
- `etl_batch_size` - Current batch size
- `etl_queue_depth` - Current queue depth

**Health endpoints**:
- `http://localhost:9090/health` - Health check (always returns 200)
- `http://localhost:9090/ready` - Readiness check (200 if all connections healthy)

### Prometheus Configuration

The included `prometheus.yml` scrapes the pipeline metrics:

```yaml
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: 'etl-pipeline'
    static_configs:
      - targets: ['etl-pipeline:9090']
```

## Project Structure

```
.
├── src/
│   └── main.go          # Main pipeline implementation
├── Dockerfile           # Multi-stage Docker build
├── docker-compose.yml   # Full stack composition
├── Makefile             # Build and run commands
├── prometheus.yml       # Prometheus configuration
├── init.sql             # PostgreSQL initialization
├── data/                # Data directory (mount point)
└── .github/workflows/
    └── pipeline.yml     # CI/CD workflow
```

## Development

### Running Tests

```bash
# Unit tests
make test

# Integration tests (requires docker-compose)
make test-integration

# With verbose output
make test-verbose
```

### Hot Reload Development

```bash
make dev  # Requires air: go install github.com/cosmtrek/air@latest
```

### Code Quality

```bash
make fmt   # Format code
make lint  # Run linter
```

## Pipeline Stages

### 1. Extract

Reads data from configured source:
- **CSV**: Reads from local CSV file
- **HTTP**: Fetches JSON array from REST API
- **PostgreSQL**: Queries directly from database

### 2. Transform

Parallel processing with configurable workers:
- **Normalization**: Trims whitespace, standardizes formats
- **Enrichment**: Adds metadata, calculates derived fields
- **Validation**: Ensures required fields exist

### 3. Load

Writes to configured destination:
- **PostgreSQL**: Batch inserts to `etl_records` table
- **stdout**: JSON lines to standard output

## Graceful Shutdown

The pipeline handles signals properly:
1. Receives SIGINT/SIGTERM
2. Stops accepting new records
3. Finishes processing current batch
4. Closes connections cleanly
5. Exits with code 0

## License

MIT