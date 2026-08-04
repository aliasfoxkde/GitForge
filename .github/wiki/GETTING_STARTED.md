# Getting Started

Welcome to the project! This guide will help you get up and running.

## Prerequisites

- Go 1.21 or later
- Git
- A GitHub account

## Quick Start

1. **Clone the repository**
   ```bash
   git clone https://github.com/aliasfoxkde/dark-factory.git
   cd dark-factory
   ```

2. **Install dependencies**
   ```bash
   go mod download
   ```

3. **Set up git hooks**
   ```bash
   make setup
   ```

4. **Run tests**
   ```bash
   make test
   ```

5. **Build**
   ```bash
   make build
   ```

## Configuration

All configuration is via environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `APP_ENV` | Environment | `development` |
| `HTTP_PORT` | HTTP port | `8080` |
| `LOG_LEVEL` | Log level | `info` |

## Next Steps

- Read the [Architecture documentation](../ARCHITECTURE.md)
- Check the [Contributing guide](../CONTRIBUTING.md)
- Browse the [Pattern Library](../PATTERN_FORMAT.md)
