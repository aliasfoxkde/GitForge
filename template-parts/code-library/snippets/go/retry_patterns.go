// Package retry provides Dark Factory standard retry patterns.
package retry

import (
	"context"
	"math"
	"math/rand"
	"time"
)

// Config configures retry behavior.
type Config struct {
	MaxAttempts int
	BaseDelay   time.Duration
	MaxDelay    time.Duration
	Jitter      bool  // Add randomness to delay
	OnRetry     func(attempt int, err error) // Called before each retry
}

// DefaultConfig returns a sensible default configuration.
func DefaultConfig() Config {
	return Config{
		MaxAttempts: 3,
		BaseDelay:   100 * time.Millisecond,
		MaxDelay:    5 * time.Second,
		Jitter:      true,
	}
}

// Do executes fn with retry logic according to config.
// Returns the last error if all attempts fail.
func Do(ctx context.Context, cfg Config, fn func() error) error {
	var lastErr error

	for attempt := 1; attempt <= cfg.MaxAttempts; attempt++ {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}

		lastErr = fn()
		if lastErr == nil {
			return nil
		}

		if attempt >= cfg.MaxAttempts {
			break
		}

		if cfg.OnRetry != nil {
			cfg.OnRetry(attempt, lastErr)
		}

		delay := calculateDelay(cfg, attempt)
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(delay):
		}
	}

	return lastErr
}

// calculateDelay computes the delay before the next retry attempt.
// Uses exponential backoff with optional jitter.
func calculateDelay(cfg Config, attempt int) time.Duration {
	delay := float64(cfg.BaseDelay) * math.Pow(2, float64(attempt-1))
	if delay > float64(cfg.MaxDelay) {
		delay = float64(cfg.MaxDelay)
	}

	if cfg.Jitter {
		// Add ±25% jitter
		jitter := delay * 0.25 * (2*rand.Float64() - 1)
		delay = delay + jitter
	}

	return time.Duration(delay)
}

// WithRetry wraps a function with retry logic.
func WithRetry(ctx context.Context, fn func() error) error {
	return Do(ctx, DefaultConfig(), fn)
}
