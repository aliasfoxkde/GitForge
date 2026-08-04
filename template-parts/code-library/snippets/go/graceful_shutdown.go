// Package shutdown provides Dark Factory standard graceful shutdown patterns.
// Use: signal handling, connection draining, timeout enforcement.
package shutdown

import (
	"context"
	"errors"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"
)

// ErrShutdownTimeout is returned when graceful shutdown times out.
var ErrShutdownTimeout = errors.New("shutdown timed out")

// Config configures the graceful shutdown behavior.
type Config struct {
	// Timeout is the maximum time allowed for graceful shutdown.
	Timeout time.Duration
	// OnShutdown is called just before shutdown begins.
	OnShutdown func()
	// OnDrain is called when connection draining begins.
	OnDrain func()
	// OnForce is called if forced termination occurs.
	OnForce func()
}

// DefaultConfig returns sensible defaults.
func DefaultConfig() Config {
	return Config{
		Timeout: 30 * time.Second,
	}
}

// Runner manages graceful shutdown of a service.
type Runner struct {
	logger *slog.Logger
	config Config
	done   chan struct{}
}

// NewRunner creates a new graceful shutdown runner.
func NewRunner(logger *slog.Logger, config Config) *Runner {
	if logger == nil {
		logger = slog.Default()
	}
	if config.Timeout == 0 {
		config = DefaultConfig()
	}
	return &Runner{
		logger: logger,
		config: config,
		done:   make(chan struct{}),
	}
}

// Run starts the shutdown runner, blocking until a signal is received.
// Returns the signal that triggered shutdown.
func (r *Runner) Run(ctx context.Context, shutdownFn func(context.Context) error) error {
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	select {
	case <-ctx.Done():
		return ctx.Err()
	case sig := <-sigCh:
		r.logger.Info("shutdown signal received", slog.String("signal", sig.String()))
		return r.Shutdown(shutdownFn)
	}
}

// Shutdown performs graceful shutdown with timeout.
func (r *Runner) Shutdown(shutdownFn func(context.Context) error) error {
	if r.config.OnShutdown != nil {
		r.config.OnShutdown()
	}

	ctx, cancel := context.WithTimeout(context.Background(), r.config.Timeout)
	defer cancel()

	done := make(chan error, 1)
	go func() {
		done <- shutdownFn(ctx)
	}()

	select {
	case <-ctx.Done():
		if r.config.OnForce != nil {
			r.config.OnForce()
		}
		r.logger.Error("graceful shutdown timed out", slog.Duration("timeout", r.config.Timeout))
		return ErrShutdownTimeout
	case err := <-done:
		if err != nil {
			r.logger.Error("shutdown error", slog.String("error", err.Error()))
		}
		r.logger.Info("shutdown complete")
		close(r.done)
		return err
	}
}
