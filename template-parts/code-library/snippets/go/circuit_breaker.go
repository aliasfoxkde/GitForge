// Package circuitbreaker implements the circuit breaker pattern for Go.
// Purpose: Prevent cascading failures by stopping requests to a failing service.
// Usage:
//   cb := circuitbreaker.New(breaker.Config{
//       MaxRequests: 3,
//       Interval:   30 * time.Second,
//       Timeout:    10 * time.Second,
//   })
//   err := cb.Execute(func() error { return callService() })
//
// Dependencies: standard library only
package circuitbreaker

import (
	"context"
	"errors"
	"sync"
	"time"
)

// State represents the circuit breaker state.
type State int

const (
	StateClosed State = iota
	StateOpen
	StateHalfOpen
)

func (s State) String() string {
	switch s {
	case StateClosed:
		return "closed"
	case StateOpen:
		return "open"
	case StateHalfOpen:
		return "half-open"
	default:
		return "unknown"
	}
}

// Errors returned by the circuit breaker.
var (
	ErrOpenState = errors.New("circuit breaker is open")
)

// Config configures the circuit breaker.
type Config struct {
	MaxRequests int           // max requests in half-open state before transitioning
	Interval    time.Duration // period of closed state to reset
	Timeout     time.Duration // duration of open state before half-open
}

// DefaultConfig returns sensible defaults.
func DefaultConfig() Config {
	return Config{
		MaxRequests: 1,
		Interval:   30 * time.Second,
		Timeout:    10 * time.Second,
	}
}

// CircuitBreaker implements the circuit breaker pattern.
type CircuitBreaker struct {
	config Config
	mu     sync.RWMutex
	state  State

	failures    int
	lastFailure time.Time

	openTime time.Time // when the circuit transitioned to open
}

// New creates a new circuit breaker.
func New(config Config) *CircuitBreaker {
	if config.MaxRequests <= 0 {
		config.MaxRequests = DefaultConfig().MaxRequests
	}
	return &CircuitBreaker{
		config: config,
		state:  StateClosed,
	}
}

// State returns the current circuit breaker state.
func (cb *CircuitBreaker) State() State {
	cb.mu.RLock()
	defer cb.mu.RUnlock()
	return cb.state
}

// Execute runs fn through the circuit breaker.
// Returns ErrOpenState if the circuit is open.
func (cb *CircuitBreaker) Execute(ctx context.Context, fn func() error) error {
	if !cb.allowRequest() {
		return ErrOpenState
	}

	err := fn()

	cb.recordResult(err)
	return err
}

// allowRequest checks if a request should be allowed through.
func (cb *CircuitBreaker) allowRequest() bool {
	cb.mu.Lock()
	defer cb.mu.Unlock()

	switch cb.state {
	case StateClosed:
		return true

	case StateOpen:
		// Check if timeout has passed
		if time.Since(cb.openTime) >= cb.config.Timeout {
			cb.transitionTo(StateHalfOpen)
			return true
		}
		return false

	case StateHalfOpen:
		// Allow limited requests through
		return cb.failures < cb.config.MaxRequests

	default:
		return false
	}
}

// recordResult records the result of a request.
func (cb *CircuitBreaker) recordResult(err error) {
	cb.mu.Lock()
	defer cb.mu.Unlock()

	if err == nil {
		cb.onSuccess()
		return
	}
	cb.onFailure()
}

func (cb *CircuitBreaker) onSuccess() {
	switch cb.state {
	case StateHalfOpen:
		cb.failures--
		if cb.failures <= 0 {
			cb.transitionToLocked(StateClosed)
		}
	case StateClosed:
		cb.failures = 0
	}
}

func (cb *CircuitBreaker) onFailure() {
	cb.lastFailure = time.Now()

	switch cb.state {
	case StateClosed:
		cb.failures++
		if cb.failures >= cb.config.MaxRequests {
			cb.transitionToLocked(StateOpen)
		}
	case StateHalfOpen:
		cb.failures++
		cb.transitionToLocked(StateOpen)
	}
}

func (cb *CircuitBreaker) transitionTo(newState State) {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.transitionToLocked(newState)
}

func (cb *CircuitBreaker) transitionToLocked(newState State) {
	cb.state = newState

	switch newState {
	case StateClosed:
		cb.failures = 0
	case StateOpen:
		cb.openTime = time.Now()
	case StateHalfOpen:
		cb.failures = 0
	}
}

// Reset manually resets the circuit breaker to closed state.
func (cb *CircuitBreaker) Reset() {
	cb.mu.Lock()
	defer cb.mu.Unlock()
	cb.transitionToLocked(StateClosed)
}
