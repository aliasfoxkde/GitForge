# CLI Tool

A production-ready CLI application template built with Go and Cobra.

## Features

- **Cobra-based CLI** - Industry-standard CLI framework with support for commands, flags, and help
- **Viper configuration** - YAML config files, environment variables, and flags
- **Cross-platform builds** - Support for Linux, macOS, and Windows (amd64 and arm64)
- **Docker support** - Multi-stage Alpine-based image with non-root user
- **GitHub Actions CI/CD** - Automated testing and multi-platform releases

## Installation

### From Release

Download the appropriate binary for your platform from the [latest release](https://github.com/example/cli-tool/releases/latest):

```bash
# Linux amd64
curl -fsSL https://github.com/example/cli-tool/releases/latest/download/cli-tool-linux-amd64 -o cli-tool
chmod +x cli-tool
sudo mv cli-tool /usr/local/bin/

# macOS arm64 (Apple Silicon)
curl -fsSL https://github.com/example/cli-tool/releases/latest/download/cli-tool-darwin-arm64 -o cli-tool
chmod +x cli-tool
sudo mv cli-tool /usr/local/bin/
```

### From Source

```bash
# Clone the repository
git clone https://github.com/example/cli-tool.git
cd cli-tool

# Build from source
make build

# Or install to ~/go/bin
make install
```

### Docker

```bash
# Build the image
make docker-build

# Run in container
make docker-run ARGS="version"
```

## Configuration

CLI Tool supports configuration via multiple sources (in order of precedence):

1. Command-line flags
2. Environment variables
3. Config file
4. Defaults

### Config File

Create a `config.yaml` in one of these locations:
- `./config.yaml`
- `./config/config.yaml`
- `$HOME/.config/cli-tool/config.yaml`
- `/etc/cli-tool/config.yaml`

Example `config.yaml`:
```yaml
verbose: false
output_format: text  # text, json, yaml
timeout: 30
environment: development
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CONFIG_PATH` | Path to config file | (none) |
| `VERBOSE` | Enable verbose output | false |
| `OUTPUT_FORMAT` | Output format | text |
| `TIMEOUT` | Timeout in seconds | 30 |
| `ENVIRONMENT` | Environment name | development |

### Command-Line Flags

```bash
cli-tool --help
cli-tool --config /path/to/config.yaml --verbose run "input data"
cli-tool -c config.yaml -v run -o json "data"
```

## Usage

### Basic Commands

```bash
# Show help
cli-tool --help

# Show version
cli-tool version
cli-tool version --json

# Run with input argument
cli-tool run "hello world"

# Run with stdin
echo "hello" | cli-tool run

# Run with file input
cli-tool run --input-file data.txt

# Verbose output
cli-tool --verbose run "data"
```

### Output Formats

```bash
# Text output (default)
cli-tool run "hello"

# JSON output
cli-tool --output-format json run "hello"
cli-tool -o json run "hello"

# YAML output
cli-tool -o yaml run "hello"
```

### Environment Examples

```bash
# Using environment variables
VERBOSE=true OUTPUT_FORMAT=json cli-tool run "test"

# With config file override
CONFIG_PATH=/etc/cli-tool/config.yaml cli-tool run "data"
```

## Development

### Prerequisites

- Go 1.21 or later
- make
- docker (optional)

### Building

```bash
# Build for current platform
make build

# Cross-compile for all platforms
make cross-build

# Build for specific platform
make build-platform PLATFORM=linux/arm64
```

### Testing

```bash
# Run all tests
make test

# Run with coverage
go test -cover ./...

# Run specific test
go test -v -run TestSpecific ./...
```

### Code Quality

```bash
# Format code
make fmt

# Run linter
make lint

# Run all checks
make check
```

### Docker Development

```bash
# Build image
make docker-build VERSION=1.0.0

# Run in container
make docker-run ARGS="run 'hello'"

# Run with volume mount for development
docker run --rm -v $(pwd):/app -w /app cli-tool:dev run "test"
```

## Project Structure

```
cli-tool/
├── src/
│   ├── main.go           # Application entry point
│   ├── cmd/
│   │   ├── root.go       # Root command and flag definitions
│   │   ├── run.go        # Run command implementation
│   │   └── version.go    # Version command implementation
│   └── config/
│       └── config.go     # Configuration management with Viper
├── Dockerfile            # Multi-stage Docker build
├── Makefile              # Build automation
├── README.md             # This file
└── .github/
    └── workflows/
        └── cli.yml       # GitHub Actions CI/CD
```

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

This project is licensed under the MIT License - see the LICENSE file for details.
