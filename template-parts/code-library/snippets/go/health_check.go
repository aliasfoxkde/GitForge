// Package health provides health check patterns for Go services.
// Purpose: Kubernetes-compatible health endpoints with readiness and liveness probes.
// Usage:
//   checker := health.NewChecker()
//   checker.AddReadiness("db", db.Ping)
//   checker.AddLiveness("goroutines", health.GoroutineCountCheck)
//   http.HandleFunc("/health/ready", checker.ReadinessHandler())
//   http.HandleFunc("/health/live", checker.LivenessHandler())
//
// Dependencies: standard library only
package health

import (
	"context"
	"encoding/json"
	"net/http"
	"runtime"
	"sync"
	"time"
)

// Status represents the overall health status.
type Status string

const (
	StatusHealthy   Status = "healthy"
	StatusUnhealthy Status = "unhealthy"
	StatusDegraded  Status = "degraded"
)

// Check defines a single health check.
type Check struct {
	Name    string
	Timeout time.Duration
	CheckFn func(context.Context) error
}

// Result holds the result of a health check.
type Result struct {
	Name     string `json:"name"`
	Status   Status `json:"status"`
	Error    string `json:"error,omitempty"`
	Duration string `json:"duration_ms,omitempty"`
}

// Response is the JSON response from health endpoints.
type Response struct {
	Status  Status   `json:"status"`
	Checks  []Result `json:"checks"`
	Time    string   `json:"timestamp"`
	Version string   `json:"version,omitempty"`
}

// Checker manages multiple health checks.
type Checker struct {
	readinessChecks []Check
	livenessChecks  []Check
	mu              sync.RWMutex
	version         string
}

// NewChecker creates a new health checker.
func NewChecker() *Checker {
	return &Checker{}
}

// WithVersion sets the service version in responses.
func (c *Checker) WithVersion(v string) *Checker {
	c.version = v
	return c
}

// AddReadiness adds a readiness check.
// Readiness checks determine if the service can handle traffic.
func (c *Checker) AddReadiness(name string, fn func(context.Context) error) {
	c.AddReadinessWithTimeout(name, 5*time.Second, fn)
}

// AddReadinessWithTimeout adds a readiness check with custom timeout.
func (c *Checker) AddReadinessWithTimeout(name string, timeout time.Duration, fn func(context.Context) error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.readinessChecks = append(c.readinessChecks, Check{
		Name:    name,
		Timeout: timeout,
		CheckFn: fn,
	})
}

// AddLiveness adds a liveness check.
// Liveness checks determine if the service should be restarted.
func (c *Checker) AddLiveness(name string, fn func(context.Context) error) {
	c.AddLivenessWithTimeout(name, 5*time.Second, fn)
}

// AddLivenessWithTimeout adds a liveness check with custom timeout.
func (c *Checker) AddLivenessWithTimeout(name string, timeout time.Duration, fn func(context.Context) error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.livenessChecks = append(c.livenessChecks, Check{
		Name:    name,
		Timeout: timeout,
		CheckFn: fn,
	})
}

// CheckReadiness runs all readiness checks.
func (c *Checker) CheckReadiness(ctx context.Context) Response {
	return c.runChecks(ctx, c.readinessChecks)
}

// CheckLiveness runs all liveness checks.
func (c *Checker) CheckLiveness(ctx context.Context) Response {
	return c.runChecks(ctx, c.livenessChecks)
}

func (c *Checker) runChecks(ctx context.Context, checks []Check) Response {
	c.mu.RLock()
	defer c.mu.RUnlock()

	results := make([]Result, 0, len(checks))
	overallStatus := StatusHealthy

	for _, check := range checks {
		result := c.runCheck(ctx, check)
		results = append(results, result)

		if result.Status == StatusUnhealthy {
			overallStatus = StatusUnhealthy
		} else if result.Status == StatusDegraded && overallStatus == StatusHealthy {
			overallStatus = StatusDegraded
		}
	}

	return Response{
		Status:  overallStatus,
		Checks:  results,
		Time:    time.Now().UTC().Format(time.RFC3339),
		Version: c.version,
	}
}

func (c *Checker) runCheck(ctx context.Context, check Check) Result {
	start := time.Now()

	type result struct {
		err error
	}

	done := make(chan result, 1)
	checkCtx, cancel := context.WithTimeout(ctx, check.Timeout)
	defer cancel()

	go func() {
		done <- result{check.CheckFn(checkCtx)}
	}()

	select {
	case <-checkCtx.Done():
		return Result{
			Name:   check.Name,
			Status: StatusUnhealthy,
			Error:  "timeout",
		}
	case r := <-done:
		duration := time.Since(start)
		if r.err != nil {
			return Result{
				Name:     check.Name,
				Status:   StatusUnhealthy,
				Error:    r.err.Error(),
				Duration: duration.String(),
			}
		}
		return Result{
			Name:     check.Name,
			Status:   StatusHealthy,
			Duration: duration.String(),
		}
	}
}

// ReadinessHandler returns an HTTP handler for readiness checks.
func (c *Checker) ReadinessHandler() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		response := c.CheckReadiness(r.Context())
		c.writeResponse(w, response, response.Status == StatusHealthy)
	}
}

// LivenessHandler returns an HTTP handler for liveness checks.
func (c *Checker) LivenessHandler() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		response := c.CheckLiveness(r.Context())
		c.writeResponse(w, response, response.Status == StatusHealthy)
	}
}

func (c *Checker) writeResponse(w http.ResponseWriter, response Response, healthy bool) {
	w.Header().Set("Content-Type", "application/json")
	if healthy {
		w.WriteHeader(http.StatusOK)
	} else {
		w.WriteHeader(http.StatusServiceUnavailable)
	}
	json.NewEncoder(w).Encode(response)
}

// Built-in health checks

// GoroutineCountCheck returns a check that fails if goroutines exceed max.
func GoroutineCountCheck(maxGoroutines int) func(context.Context) error {
	return func(ctx context.Context) error {
		count := runtime.NumGoroutine()
		if count > maxGoroutines {
			return &HealthCheckError{
				Message: "too many goroutines",
				Details: map[string]int{"count": count, "max": maxGoroutines},
			}
		}
		return nil
	}
}

// HealthCheckError represents a health check failure with details.
type HealthCheckError struct {
	Message string
	Details map[string]int
}

func (e *HealthCheckError) Error() string {
	return e.Message
}
