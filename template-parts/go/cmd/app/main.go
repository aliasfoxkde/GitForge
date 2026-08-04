// Package main is the entry point for the application.
package main

import (
	"context"
	"errors"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"
)

// ErrShutdown is returned when graceful shutdown times out.
var ErrShutdown = errors.New("shutdown timed out")

// App holds the application state and dependencies.
type App struct {
	logger *slog.Logger
	cfg    *Config
}

// NewApp creates a new application instance.
func NewApp(logger *slog.Logger, cfg *Config) *App {
	return &App{
		logger: logger,
		cfg:    cfg,
	}
}

// Run starts the application and blocks until shutdown.
func (a *App) Run(ctx context.Context) error {
	a.logger.Info("starting application",
		slog.String("version", a.cfg.Version),
		slog.String("env", a.cfg.Env),
	)

	// TODO: Start servers, workers, etc.
	// e.g., startHTTPServer(a.cfg.HTTPPort, a.handler())

	<-ctx.Done()
	return a.Shutdown(10 * time.Second)
}

// Shutdown gracefully stops the application.
func (a *App) Shutdown(timeout time.Duration) error {
	a.logger.Info("shutting down application")

	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	done := make(chan error, 1)
	go func() {
		// TODO: Drain connections, finish pending work
		done <- nil
	}()

	select {
	case <-ctx.Done():
		return ErrShutdown
	case err := <-done:
		return err
	}
}

func main() {
	// Initialize structured logger
	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	}))
	slog.SetDefault(logger)

	// Load configuration
	cfg := LoadConfig()

	app := NewApp(logger, cfg)

	// Graceful shutdown on SIGINT/SIGTERM
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
		<-sig
		logger.Info("signal received, initiating shutdown")
		cancel()
	}()

	if err := app.Run(ctx); err != nil {
		logger.Error("application error", slog.String("error", err.Error()))
		os.Exit(1)
	}

	logger.Info("application stopped cleanly")
}
