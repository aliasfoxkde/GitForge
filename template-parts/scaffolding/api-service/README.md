# API Service

A production-ready REST API microservice scaffold built with Go and Gin.

## Features

- **HTTP API** with Gin framework
- **Authentication middleware** (Authorization header validation)
- **Rate limiting** (in-memory sliding window)
- **Structured logging** (JSON logs via slog)
- **Graceful shutdown**
- **Health check endpoint**
- **Docker & Docker Compose** support
- **GitHub Actions CI/CD**

## Quick Start

### Prerequisites

- Go 1.24+
- Docker & Docker Compose
- PostgreSQL 16+ (via Docker Compose)
- Redis 7+ (via Docker Compose)

### Local Development

```bash
# Run with docker-compose (recommended)
make docker-run

# Or run locally
make run
```

### Run Tests

```bash
make test
```

### Build Docker Image

```bash
make docker-build
```

## Environment Variables

| Variable      | Default                                                    | Description               |
|---------------|------------------------------------------------------------|---------------------------|
| `APP_HOST`    | `0.0.0.0`                                                  | Host to bind to           |
| `APP_PORT`    | `8080`                                                     | Port to listen on         |
| `APP_ENV`     | `development`                                              | `development` or `production` |
| `APP_LOG_LEVEL` | `info`                                                   | Log level (debug, info, warn, error) |
| `APP_DB_URL`  | `postgres://postgres:password@localhost:5432/apidb?sslmode=disable` | Database connection URL |

## API Endpoints

### Health Check

```
GET /health
```

Returns service health status.

### Items (requires Authorization header)

```
GET    /api/v1/items        # List all items
GET    /api/v1/items/:id    # Get item by ID
POST   /api/v1/items        # Create item
PUT    /api/v1/items/:id    # Update item
DELETE /api/v1/items/:id    # Delete item
```

#### Create Item Request

```json
POST /api/v1/items
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "My Item",
  "description": "Item description"
}
```

#### Update Item Request

```json
PUT /api/v1/items/:id
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "Updated Name"
}
```

## Configuration File

Place `config.yaml` in one of these locations (searched in order):

1. `/etc/api-service/config.yaml`
2. `~/.api-service/config.yaml`
3. `./config.yaml` (project root)

Example `config.yaml`:

```yaml
host: "0.0.0.0"
port: 8080
env: "development"
log_level: "info"
db_url: "postgres://postgres:password@localhost:5432/apidb?sslmode=disable"
```

## Docker

### Build Image

```bash
docker build -t api-service:latest .
```

### Run with Docker Compose

```bash
docker-compose up -d
```

### Stop Services

```bash
docker-compose down
```

### Clean Volumes

```bash
docker-compose down -v
```

## Project Structure

```
api-service/
├── src/
│   ├── main.go          # Application entry point
│   ├── api/
│   │   ├── router.go    # Route registration
│   │   ├── handlers.go  # HTTP handlers
│   │   └── middleware.go # Auth, logging, rate limiting
│   ├── models/
│   │   └── types.go     # Data models
│   ├── services/
│   │   └── business.go  # Business logic
│   └── config/
│       └── config.go    # Configuration loading
├── Dockerfile
├── docker-compose.yml
├── Makefile
└── README.md
```

## License

MIT