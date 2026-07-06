// Package worker provides the job processor and queue consumer.
package worker

import (
	"context"
	"encoding/json"
	"log/slog"
	"time"
)

// Job represents a generic job message from the queue.
type Job struct {
	ID          string          `json:"id"`
	Type        string          `json:"type"`
	Payload     json.RawMessage `json:"payload"`
	Retries     int             `json:"retries"`
	MaxRetries  int             `json:"max_retries"`
	StartedAt   *time.Time      `json:"started_at,omitempty"`
	CompletedAt *time.Time      `json:"completed_at,omitempty"`
}

// JobResult holds the outcome of processing a job.
type JobResult struct {
	JobID      string
	Success    bool
	Output     interface{}
	Error      error
	Duration   time.Duration
	ShouldRetry bool
}

// Processor handles the actual work of processing a job.
type Processor struct {
	logger *slog.Logger
}

// NewProcessor creates a new job processor.
func NewProcessor(logger *slog.Logger) *Processor {
	return &Processor{
		logger: logger,
	}
}

// ProcessMessage processes a single job and returns the result.
// It is safe to call concurrently; the processor can handle multiple
// jobs in parallel. Implement your business logic inside this function.
func (p *Processor) ProcessMessage(ctx context.Context, job *Job) *JobResult {
	start := time.Now()

	logger := p.logger.With(
		"job_id", job.ID,
		"job_type", job.Type,
		"retries", job.Retries,
	)

	logger.Info("processing job started")

	// Check context cancellation
	select {
	case <-ctx.Done():
		logger.Warn("job processing cancelled")
		return &JobResult{
			JobID:       job.ID,
			Success:     false,
			Error:      ctx.Err(),
			Duration:   time.Since(start),
			ShouldRetry: false,
		}
	default:
	}

	// Dispatch based on job type
	var result *JobResult
	switch job.Type {
	case "example":
		result = p.handleExampleJob(ctx, job)
	default:
		logger.Warn("unknown job type, acknowledging to prevent requeue")
		result = &JobResult{
			JobID:      job.ID,
			Success:   true,
			Output:    "unknown job type acknowledged",
			Duration:  time.Since(start),
		}
	}

	logger.Info("job processing completed",
		"job_id", job.ID,
		"success", result.Success,
		"duration_ms", result.Duration.Milliseconds(),
		"should_retry", result.ShouldRetry,
	)

	return result
}

// handleExampleJob demonstrates how to process a specific job type.
// Replace this with your actual business logic.
func (p *Processor) handleExampleJob(ctx context.Context, job *Job) *JobResult {
	// Example payload structure: {"message": "hello"}
	type ExamplePayload struct {
		Message string `json:"message"`
	}

	var payload ExamplePayload
	if err := json.Unmarshal(job.Payload, &payload); err != nil {
		return &JobResult{
			JobID:       job.ID,
			Success:    false,
			Error:      err,
			Duration:   time.Since(time.Now()),
			ShouldRetry: false,
		}
	}

	// Simulate work (replace with real work)
	select {
	case <-ctx.Done():
		return &JobResult{
			JobID:       job.ID,
			Success:    false,
			Error:      ctx.Err(),
			Duration:   time.Since(time.Now()),
			ShouldRetry: true,
		}
	case <-time.After(100 * time.Millisecond):
		// Work complete
	}

	return &JobResult{
		JobID:   job.ID,
		Success: true,
		Output: map[string]string{
			"processed": payload.Message,
			"status":    "ok",
		},
		Duration:   time.Since(time.Now()),
		ShouldRetry: false,
	}
}
