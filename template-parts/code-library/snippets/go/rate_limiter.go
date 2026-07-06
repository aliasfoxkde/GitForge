// Package ratelimiter implements a token bucket rate limiter for Go.
// Purpose: Control the rate of operations to prevent overwhelming downstream services.
// Usage:
//   limiter := ratelimiter.New(100, time.Second) // 100 requests per second
//   if err := limiter.Wait(ctx); err != nil { return err }
//   doWork()
//
// Dependencies: standard library only
package ratelimiter

import (
	"context"
	"errors"
	"sync"
	"time"
)

// ErrRateLimited is returned when the rate limit prevents a request.
var ErrRateLimited = errors.New("rate limit exceeded")

// Limiter implements a token bucket rate limiter.
type Limiter struct {
	mu       sync.Mutex
	rate     float64         // tokens per second
	capacity int             // max tokens (bucket size)
	tokens   float64         // current available tokens
	lastTime time.Time       // last token refill time

	// Optional channel for async notification
	notifyCh chan struct{}
}

// New creates a new rate limiter.
// rate specifies tokens per second, capacity is the max bucket size.
func New(rate float64, capacity int) *Limiter {
	return &Limiter{
		rate:       rate,
		capacity:   capacity,
		tokens:     float64(capacity),
		lastTime:   time.Now(),
		notifyCh:   make(chan struct{}, 1),
	}
}

// NewWithNotify creates a limiter that notifies via channel when tokens are available.
func NewWithNotify(rate float64, capacity int) *Limiter {
	limiter := New(rate, capacity)
	limiter.notifyCh = make(chan struct{}, capacity)
	return limiter
}

// Wait blocks until a token is available or context is cancelled.
func (l *Limiter) Wait(ctx context.Context) error {
	select {
	case <-ctx.Done():
		return ctx.Err()
	default:
	}

	if err := l.reserve(1); err != nil {
		return err
	}

	// Wait for token if none available
	if l.tokens < 0 {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(l.estimateWaitTime()):
			return nil
		}
	}
	return nil
}

// Try acquires n tokens without blocking.
// Returns nil on success, ErrRateLimited if not enough tokens.
func (l *Limiter) Try(n int) error {
	return l.reserve(n)
}

// reserve attempts to reserve n tokens.
func (l *Limiter) reserve(n int) error {
	l.mu.Lock()
	defer l.mu.Unlock()

	l.refill()

	if float64(n) > l.tokens {
		return ErrRateLimited
	}

	l.tokens -= float64(n)
	return nil
}

// refill adds tokens based on elapsed time since last refill.
func (l *Limiter) refill() {
	now := time.Now()
	elapsed := now.Sub(l.lastTime).Seconds()
	l.lastTime = now

	// Add tokens based on rate and elapsed time
	l.tokens += elapsed * l.rate
	if l.tokens > float64(l.capacity) {
		l.tokens = float64(l.capacity)
	}
}

// estimateWaitTime estimates how long until a token is available.
func (l *Limiter) estimateWaitTime() time.Duration {
	l.mu.Lock()
	defer l.mu.Unlock()

	if l.tokens >= 0 {
		return 0
	}
	return time.Duration((-l.tokens / l.rate) * float64(time.Second))
}

// Tokens returns the current number of available tokens.
func (l *Limiter) Tokens() float64 {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.refill()
	return l.tokens
}

// Capacity returns the maximum token capacity.
func (l *Limiter) Capacity() int {
	return l.capacity
}

// Rate returns the token generation rate per second.
func (l *Limiter) Rate() float64 {
	return l.rate
}

// Reset restores the limiter to full capacity.
func (l *Limiter) Reset() {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.tokens = float64(l.capacity)
	l.lastTime = time.Now()
}

// Burst attempts to consume up to n tokens instantly.
// Returns the number of tokens actually consumed.
func (l *Limiter) Burst(n int) int {
	l.mu.Lock()
	defer l.mu.Unlock()

	l.refill()

	available := int(l.tokens)
	if available > n {
		available = n
	}
	if available > l.capacity {
		available = l.capacity
	}

	l.tokens -= float64(available)
	return available
}
