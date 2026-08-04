// Package context provides Dark Factory standard context propagation patterns.
package context

import (
	"context"
	"errors"
	"time"
)

// ─── Context Values ───────────────────────────────────────────────────────────

type contextKey string

const (
	// KeyRequestID is the context key for request ID.
	KeyRequestID contextKey = "request_id"
	// KeyUserID is the context key for user ID.
	KeyUserID contextKey = "user_id"
	// KeyLogger is the context key for structured logger.
	KeyLogger contextKey = "logger"
)

// WithRequestID adds a request ID to the context.
func WithRequestID(ctx context.Context, requestID string) context.Context {
	return context.WithValue(ctx, KeyRequestID, requestID)
}

// GetRequestID retrieves the request ID from context.
func GetRequestID(ctx context.Context) (string, bool) {
	v, ok := ctx.Value(KeyRequestID).(string)
	return v, ok
}

// ─── Timeout Patterns ────────────────────────────────────────────────────────

// WithSoftTimeout returns a context that cancels after the given duration,
// but ONLY if the parent context is still valid. This prevents cancelling
// work that the parent has already completed.
func WithSoftTimeout(parent context.Context, duration time.Duration) (context.Context, context.CancelFunc) {
	return context.WithTimeout(parent, duration)
}

// ─── Error Propagation ────────────────────────────────────────────────────────

// ErrContextCancelled is returned when a context is cancelled.
var ErrContextCancelled = errors.New("context cancelled")

// ErrContextDeadlineExceeded is returned when a context deadline is exceeded.
var ErrContextDeadlineExceeded = errors.New("context deadline exceeded")

// PropagateContextError converts a context error to a domain error.
func PropagateContextError(ctx context.Context) error {
	select {
	case <-ctx.Done():
		if errors.Is(ctx.Err(), context.Canceled) {
			return ErrContextCancelled
		}
		if errors.Is(ctx.Err(), context.DeadlineExceeded) {
			return ErrContextDeadlineExceeded
		}
		return ctx.Err()
	default:
		return nil
	}
}
