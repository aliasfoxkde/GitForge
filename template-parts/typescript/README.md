# Project Name

A production-ready TypeScript project with structured API, models, services, and utilities.

## Project Structure

```
src/
├── project-name/
│   ├── api/          # HTTP handlers and route definitions
│   ├── models/       # TypeScript interfaces and types
│   ├── services/     # Business logic layer
│   └── utils/        # Logging and utility functions
└── index.ts          # Main entry point
```

## Features

- **TypeScript** with strict mode enabled
- **Vitest** for unit and integration testing
- **Playwright** for end-to-end testing
- **ESLint** with TypeScript rules
- **Prettier** for code formatting
- **Structured logging** with configurable log levels

## Prerequisites

- Node.js >= 20
- npm >= 10

## Getting Started

### Installation

```bash
npm install
```

### Development

```bash
# Run type checking
npm run typecheck

# Run linting
npm run lint

# Format code
npm run format

# Run tests
npm run test
```

### Building

```bash
npm run build
```

This compiles TypeScript from `src/` to `dist/`.

### Testing

```bash
# Run all tests
npm test

# Run unit tests only
npm run test:unit

# Run integration tests only
npm run test:integration

# Run e2e tests only
npm run test:e2e

# Run tests with coverage
npm run coverage
```

### Makefile Commands

```bash
make install        # Install dependencies
make test          # Run all tests
make test:unit     # Run unit tests
make lint          # Run ESLint
make format        # Format code
make typecheck     # Type check
make build         # Build project
make coverage      # Run with coverage
make clean         # Remove artifacts
```

## API Handlers

The API handlers provide HTTP request processing:

- `handleHealth()` - Returns service health status
- `handleListResources()` - Lists all resources
- `handleCreateResource(request)` - Creates a new resource
- `handleGetResource(id)` - Gets a resource by ID
- `handleDeleteResource(id)` - Deletes a resource by ID

## Business Service

The `BusinessService` class provides the core business logic:

- Resource CRUD operations
- Health checking
- Resource counting and management

## Logging

Uses structured logging with configurable levels:

```typescript
import { createLogger, setLogLevel } from './utils/logging.js';

const logger = createLogger('module-name');
logger.info('Message', { key: 'value' });

setLogLevel('debug'); // Enable debug logging
```

## License

ISC
