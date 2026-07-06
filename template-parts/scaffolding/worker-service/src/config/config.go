// Package config loads and validates environment configuration.
package config

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"time"
)

// Config holds all environment-driven configuration.
type Config struct {
	// AMQPURL is the RabbitMQ connection URL.
	// If set, takes precedence over KAFKA_BROKERS.
	// Example: amqp://user:pass@host:5672/
	AMQPURL string

	// KAFKABrokers is the list of Kafka brokers (comma-separated).
	// Used when AMQP_URL is not set.
	KAFKABrokers string

	// WorkerCount is the number of concurrent workers.
	WorkerCount int

	// LogLevel sets the logger level (debug, info, warn, error).
	LogLevel string

	// ShutdownTimeout is the maximum seconds to wait for graceful shutdown.
	ShutdownTimeout int

	// DatabaseDSN is the optional PostgreSQL connection string.
	DatabaseDSN string

	// MetricsAddr is the address to expose Prometheus metrics (e.g., :9090).
	MetricsAddr string
}

// Load reads configuration from environment variables with sensible defaults.
func Load() (*Config, error) {
	ctx := context.Background()

	cfg := &Config{
		AMQPURL:         getEnv("AMQP_URL", ""),
		KAFKABrokers:    getEnv("KAFKA_BROKERS", ""),
		WorkerCount:    4,
		LogLevel:       "info",
		ShutdownTimeout: 30,
		DatabaseDSN:    getEnv("DATABASE_DSN", ""),
		MetricsAddr:    getEnv("METRICS_ADDR", ":9090"),
	}

	if v := getEnv("WORKER_COUNT", ""); v != "" {
		n, err := strconv.Atoi(v)
		if err != nil {
			return nil, fmt.Errorf("WORKER_COUNT must be an integer: %w", err)
		}
		if n < 1 || n > 256 {
			return nil, fmt.Errorf("WORKER_COUNT must be between 1 and 256, got %d", n)
		}
		cfg.WorkerCount = n
	}

	if v := getEnv("LOG_LEVEL", ""); v != "" {
		switch v {
		case "debug", "info", "warn", "error":
			cfg.LogLevel = v
		default:
			return nil, fmt.Errorf("LOG_LEVEL must be one of debug, info, warn, error; got %q", v)
		}
	}

	if v := getEnv("SHUTDOWN_TIMEOUT", ""); v != "" {
		n, err := strconv.Atoi(v)
		if err != nil {
			return nil, fmt.Errorf("SHUTDOWN_TIMEOUT must be an integer: %w", err)
		}
		if n < 1 {
			return nil, fmt.Errorf("SHUTDOWN_TIMEOUT must be at least 1 second")
		}
		cfg.ShutdownTimeout = n
	}

	// Validate that at least one queue backend is configured
	if cfg.AMQPURL == "" && cfg.KAFKABrokers == "" {
		// Default to a local RabbitMQ for development
		cfg.AMQPURL = "amqp://guest:guest@localhost:5672/"
	}

	_ = ctx // placeholder for future context-based config

	return cfg, nil
}

// getEnv returns the environment variable or a default value.
func getEnv(key, defaultValue string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return defaultValue
}

// ShutdownDuration returns the shutdown timeout as a time.Duration.
func (c *Config) ShutdownDuration() time.Duration {
	return time.Duration(c.ShutdownTimeout) * time.Second
}
