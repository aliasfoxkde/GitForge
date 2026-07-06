// Package logger provides structured logging configuration.
package logger

import (
	"io"
	"log/slog"
	"os"
)

// Config holds logger configuration.
type Config struct {
	Level  string // debug, info, warn, error
	Format string // json, text
}

// New creates a new structured logger.
func New(config *Config) *slog.Logger {
	var level slog.Level
	switch config.Level {
	case "debug":
		level = slog.LevelDebug
	case "warn":
		level = slog.LevelWarn
	case "error":
		level = slog.LevelError
	default:
		level = slog.LevelInfo
	}

	opts := &slog.HandlerOptions{
		Level: level,
	}

	var handler slog.Handler
	if config.Format == "text" {
		handler = slog.NewTextHandler(os.Stdout, opts)
	} else {
		handler = slog.NewJSONHandler(os.Stdout, opts)
	}

	return slog.New(handler)
}

// NewWithWriter creates a logger with a custom writer (e.g., for testing).
func NewWithWriter(w io.Writer, config *Config) *slog.Logger {
	var level slog.Level
	switch config.Level {
	case "debug":
		level = slog.LevelDebug
	case "warn":
		level = slog.LevelWarn
	case "error":
		level = slog.LevelError
	default:
		level = slog.LevelInfo
	}

	opts := &slog.HandlerOptions{
		Level: level,
	}

	var handler slog.Handler
	if config.Format == "text" {
		handler = slog.NewTextHandler(w, opts)
	} else {
		handler = slog.NewJSONHandler(w, opts)
	}

	return slog.New(handler)
}

// Default returns a production-ready JSON logger.
func Default() *slog.Logger {
	return New(&Config{
		Level:  "info",
		Format: "json",
	})
}

// Debug returns a development logger with debug level and text format.
func Debug() *slog.Logger {
	return New(&Config{
		Level:  "debug",
		Format: "text",
	})
}
