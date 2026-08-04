# project_name

A Rust project built with axum, tokio, and tower.

## Structure

```
src/
├── lib.rs           # main library exports
├── api/
│   ├── mod.rs
│   └── handlers.rs  # HTTP handlers
├── models/
│   ├── mod.rs
│   └── types.rs     # structs, enums
├── services/
│   ├── mod.rs
│   └── business.rs  # business logic
└── utils/
    ├── mod.rs
    └── logging.rs   # tracing setup
```

## Prerequisites

- Rust 1.75+ (via rust-toolchain.toml)
- cargo

## Commands

| Command | Description |
|---------|-------------|
| `make build` | Build the project |
| `make test` | Run all tests |
| `make test-unit` | Run unit tests only |
| `make test-integration` | Run integration tests only |
| `make clippy` | Run clippy lints |
| `make fmt` | Format code |
| `make check` | Format, lint, and build |
| `make clean` | Remove build artifacts |
| `make run` | Run the project |

## License

MIT
