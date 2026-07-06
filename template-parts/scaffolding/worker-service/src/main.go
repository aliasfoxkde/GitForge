// Worker Service Main Entry Point
package main

import (
	"context"
	"log/slog"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	"worker-service/src/config"
	"worker-service/src/worker"
)

func main() {
	// Initialize structured logger
	logHandler := slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	})
	logger := slog.New(logHandler)
	slog.SetDefault(logger)

	cfg, err := config.Load()
	if err != nil {
		logger.Error("failed to load config", "error", err)
		os.Exit(1)
	}

	logger.Info("starting worker service",
		"worker_count", cfg.WorkerCount,
		"log_level", cfg.LogLevel,
		"shutdown_timeout", cfg.ShutdownTimeout,
	)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Set up graceful shutdown
	shutdownTimeout := time.Duration(cfg.ShutdownTimeout) * time.Second
	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)

	var wg sync.WaitGroup

	// Create and connect the queue consumer
	consumer, err := worker.NewConsumer(ctx, cfg)
	if err != nil {
		logger.Error("failed to create consumer", "error", err)
		os.Exit(1)
	}

	if err := consumer.Connect(); err != nil {
		logger.Error("failed to connect to queue", "error", err)
		os.Exit(1)
	}

	// Create the job processor
	processor := worker.NewProcessor(logger)

	// Start workers
	_ = consumer.StartWorkers(cfg.WorkerCount, processor)

	// Wait for shutdown signal
	select {
	case sig := <-quit:
		logger.Info("received shutdown signal", "signal", sig)
	case <-ctx.Done():
		logger.Info("context cancelled")
	}

	logger.Info("initiating graceful shutdown")

	// Cancel context to stop workers
	cancel()

	// Give in-flight messages time to complete
	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), shutdownTimeout)
	defer shutdownCancel()

	// Close consumer (stops new messages, waits for in-flight)
	closeErr := consumer.Close(shutdownCtx)

	// Wait for workers to finish
	wg.Wait()

	if closeErr != nil {
		logger.Error("error during close", "error", closeErr)
		os.Exit(1)
	}

	logger.Info("worker service stopped gracefully")
}
