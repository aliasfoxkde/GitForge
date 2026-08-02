# GitForge Deployment Guide

This guide covers deploying GitForge using Docker Compose for self-hosted Git with CI/CD.

## Prerequisites

- Docker Engine 24.0+
- Docker Compose 2.20+
- 4GB RAM minimum (8GB recommended)
- 20GB disk space

## Quick Start

1. Clone the repository:
```bash
git clone https://github.com/your-org/gitforge.git
cd gitforge
```

2. Copy the configuration file:
```bash
cp config.toml.example config.toml
# Edit config.toml with your settings
```

3. Set a secure JWT secret:
```bash
export JWT_SECRET=$(openssl rand -base64 32)
```

4. Start the services:
```bash
docker-compose up -d
```

5. Verify health:
```bash
curl http://localhost:42780/health
```

## Services

### API Gateway (port 42780)
REST API for GitForge. All client interaction goes through this service.

### CI Orchestrator (port 42781)
Processes pipeline events and orchestrates job execution. The scheduler HTTP API runs within this service.

### Runner
Executes CI jobs in Docker containers. Multiple runners can be deployed horizontally. Connects to CI service at `http://ci:42781`.

### Git Server (ports 42782 HTTP, 42022 SSH)
Handles Git SSH and HTTP protocols. SSH support is pending implementation.

## Configuration

Edit `config.toml` to customize:

| Section | Key | Description |
|---------|-----|-------------|
| `server` | `port` | API port |
| `database` | `url` | SQLite or PostgreSQL URL |
| `auth` | `jwt_secret` | JWT signing secret |
| `runner` | `capacity` | Concurrent job slots |

## Scaling Runners

Add more runners by scaling the service:
```bash
docker-compose up -d --scale runner=3
```

Or enable the second runner in docker-compose.yml:
```yaml
runner-2:
  deploy:
    replicas: 1  # Enable
```

## Database Migration

For production, use PostgreSQL:
```toml
[database]
url = "postgres://gitforge:password@postgres:5432/gitforge"
```

## Monitoring

Prometheus metrics available at `http://localhost:42780/metrics`.

Key metrics:
- `gitforge_http_requests_total` - HTTP request counts
- `gitforge_job_duration_seconds` - Job execution time
- `gitforge_runners_online` - Active runners

## Troubleshooting

### Runner can't connect to scheduler
```bash
docker-compose logs runner
# Check SCHEDULER_URL environment variable
```

### Jobs stuck in queue
```bash
docker-compose logs scheduler
docker-compose logs ci
```

### Database locked
SQLite doesn't support concurrent writes. For multi-runner setups, use PostgreSQL.

## Production Checklist

- [ ] Change JWT secret
- [ ] Use PostgreSQL instead of SQLite
- [ ] Configure CORS origins
- [ ] Set up TLS reverse proxy
- [ ] Enable rate limiting
- [ ] Configure backup for database volume
