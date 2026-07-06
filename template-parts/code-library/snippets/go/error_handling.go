// Package errors provides Dark Factory standard error handling patterns.
package errors

import (
	"context"
	"errors"
	"fmt"
)

// ─── Sentinel Errors ──────────────────────────────────────────────────────────
// Define package-level sentinel errors. These are comparable via errors.Is().

// ErrNotFound is returned when a requested resource does not exist.
var ErrNotFound = errors.New("not found")

// ErrPermissionDenied is returned when the operation is not permitted.
var ErrPermissionDenied = errors.New("permission denied")

// ErrInvalidInput is returned when input validation fails.
var ErrInvalidInput = errors.New("invalid input")

// ErrTimeout is returned when an operation times out.
var ErrTimeout = errors.New("operation timed out")

// ErrCancelled is returned when an operation is cancelled.
var ErrCancelled = errors.New("operation cancelled")

// ─── Wrapped Errors ───────────────────────────────────────────────────────────
// Use fmt.Errorf with %w for contextual wrapping.

// NotFoundError represents a specific not-found condition.
type NotFoundError struct {
	Resource string
	ID       string
}

func (e *NotFoundError) Error() string {
	return fmt.Sprintf("%s not found: %s", e.Resource, e.ID)
}

func (e *NotFoundError) Is(target error) bool {
	return target == ErrNotFound
}

// NewNotFoundError creates a new NotFoundError.
func NewNotFoundError(resource, id string) error {
	return &NotFoundError{Resource: resource, ID: id}
}

// ─── Error Checking Patterns ─────────────────────────────────────────────────

// CheckNotFound checks if err is a not-found error using errors.Is().
// This is the RECOMMENDED pattern for error checking.
//
//   if errors.Is(err, ErrNotFound) { ... }
//
// NOT: if err == ErrNotFound { ... }  // WRONG — fails for wrapped errors

// CheckContext checks for context cancellation or deadline exceeded.
func CheckContext(ctx context.Context) error {
	if errors.Is(ctx.Err(), context.Canceled) {
		return ErrCancelled
	}
	if errors.Is(ctx.Err(), context.DeadlineExceeded) {
		return ErrTimeout
	}
	return nil
}

// MapError maps internal errors to user-facing errors without leaking details.
// Always log the real error; return only what the user should see.
func MapError(err error) error {
	// Log real error here (in production, use structured logging)
	// slog.Error("internal error", "error", err)

	switch {
	case errors.Is(err, ErrNotFound):
		return fmt.Errorf("the requested item could not be found")
	case errors.Is(err, ErrPermissionDenied):
		return fmt.Errorf("you do not have permission to perform this action")
	case errors.Is(err, ErrInvalidInput):
		return fmt.Errorf("invalid input provided")
	case errors.Is(err, ErrTimeout):
		return fmt.Errorf("the operation took too long; please try again")
	default:
		return fmt.Errorf("an unexpected error occurred")
	}
}
