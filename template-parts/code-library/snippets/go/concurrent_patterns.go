// Package concurrent provides Dark Factory standard concurrency patterns.
package concurrent

import (
	"context"
	"sync"
	"time"
)

// Task represents a unit of work.
type Task func(context.Context) error

// Runner runs tasks with bounded concurrency.
type Runner struct {
	concurrency int
	wg          sync.WaitGroup
	errCh       chan error
	ctx         context.Context
	cancel      context.CancelFunc
}

// NewRunner creates a new bounded concurrency runner.
func NewRunner(concurrency int) *Runner {
	ctx, cancel := context.WithCancel(context.Background())
	return &Runner{
		concurrency: concurrency,
		errCh:      make(chan error, concurrency),
		ctx:        ctx,
		cancel:     cancel,
	}
}

// Run executes tasks with bounded concurrency.
// Returns after all tasks complete or on first critical error.
func (r *Runner) Run(tasks []Task) []error {
	sem := make(chan struct{}, r.concurrency)
	errs := make([]error, 0)
	var mu sync.Mutex

	for _, task := range tasks {
		select {
		case <-r.ctx.Done():
			break
		case sem <- struct{}{}:
		}

		r.wg.Add(1)
		go func(t Task) {
			defer r.wg.Done()
			defer func() { <-sem }()

			if err := t(r.ctx); err != nil {
				mu.Lock()
				errs = append(errs, err)
				mu.Unlock()
			}
		}(task)
	}

	r.wg.Wait()
	return errs
}

// Cancel cancels all in-flight tasks.
func (r *Runner) Cancel() {
	r.cancel()
}

// WorkerPool creates a fixed-size pool of workers.
type WorkerPool struct {
	workers   int
	taskCh    chan Task
	resultCh  chan error
	ctx       context.Context
	cancel    context.CancelFunc
}

// NewWorkerPool creates a new worker pool.
func NewWorkerPool(workers int, buffer int) *WorkerPool {
	ctx, cancel := context.WithCancel(context.Background())
	return &WorkerPool{
		workers:  workers,
		taskCh:   make(chan Task, buffer),
		resultCh: make(chan error, buffer),
		ctx:      ctx,
		cancel:   cancel,
	}
}

// Start starts the worker pool.
func (p *WorkerPool) Start() {
	for i := 0; i < p.workers; i++ {
		go func() {
			for task := range p.taskCh {
				select {
				case <-p.ctx.Done():
					return
				case p.resultCh <- task(p.ctx):
				}
			}
		}()
	}
}

// Submit submits a task to the pool.
func (p *WorkerPool) Submit(task Task) {
	p.taskCh <- task
}

// Results returns the results channel.
func (p *WorkerPool) Results() <-chan error {
	return p.resultCh
}

// Stop stops the worker pool.
func (p *WorkerPool) Stop() {
	close(p.taskCh)
	p.cancel()
}

// Retry runs a task with exponential backoff.
func Retry(ctx context.Context, maxAttempts int, baseDelay time.Duration, fn func() error) error {
	var err error
	for attempt := 1; attempt <= maxAttempts; attempt++ {
		if err = fn(); err == nil {
			return nil
		}
		if attempt >= maxAttempts {
			break
		}
		delay := baseDelay * time.Duration(1<<(attempt-1))
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(delay):
		}
	}
	return err
}
