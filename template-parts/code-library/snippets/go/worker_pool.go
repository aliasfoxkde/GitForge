// Package workerpool implements a fixed-size worker pool with fan-in for Go.
// Purpose: Execute tasks concurrently with bounded parallelism and collect results.
// Usage:
//   pool := workerpool.New(4, 100) // 4 workers, buffer for 100 tasks
//   pool.Start()
//   for _, task := range tasks {
//       pool.Submit(task)
//   }
//   pool.Stop()
//   for result := range pool.Results() {
//       // process result
//   }
//
// Dependencies: standard library only
package workerpool

import (
	"context"
	"sync"
)

// Task represents a unit of work that returns a result or error.
type Task func() (any, error)

// Result represents the outcome of a task.
type Result struct {
	Value any
	Err   error
}

// Pool manages a pool of workers that process tasks and collect results.
type Pool struct {
	workers   int
	taskCh    chan Task
	resultCh  chan Result
	ctx       context.Context
	cancel    context.CancelFunc
	wg        sync.WaitGroup
	started   bool
	mu        sync.RWMutex
}

// New creates a new worker pool with the specified number of workers.
// buffer sets the capacity of the task channel.
func New(workers int, buffer int) *Pool {
	if workers <= 0 {
		workers = 1
	}
	if buffer <= 0 {
		buffer = 1
	}

	ctx, cancel := context.WithCancel(context.Background())
	return &Pool{
		workers:  workers,
		taskCh:   make(chan Task, buffer),
		resultCh: make(chan Result, buffer),
		ctx:      ctx,
		cancel:   cancel,
	}
}

// Start begins processing tasks with the configured number of workers.
func (p *Pool) Start() {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.started {
		return
	}
	p.started = true

	for i := 0; i < p.workers; i++ {
		p.wg.Add(1)
		go p.worker()
	}
}

// worker processes tasks from the channel.
func (p *Pool) worker() {
	defer p.wg.Done()

	for {
		select {
		case <-p.ctx.Done():
			return
		case task, ok := <-p.taskCh:
			if !ok {
				return
			}
			p.executeTask(task)
		}
	}
}

// executeTask runs a single task and sends the result.
func (p *Pool) executeTask(task Task) {
	var result Result

	select {
	case <-p.ctx.Done():
		result = Result{Err: p.ctx.Err()}
	case p.resultCh <- func() Result {
		value, err := task()
		return Result{Value: value, Err: err}
	}():
	}
}

// Submit adds a task to the pool.
// Blocks if the task channel is full.
func (p *Pool) Submit(task Task) {
	select {
	case <-p.ctx.Done():
		return
	case p.taskCh <- task:
	}
}

// SubmitAndWait submits a task and waits for its result.
func (p *Pool) SubmitAndWait(task Task) Result {
	done := make(chan Result, 1)

	select {
	case <-p.ctx.Done():
		return Result{Err: p.ctx.Err()}
	case p.taskCh <- func() (any, error) {
		result := task()
		done <- result
		return nil, nil
	}:
	}

	select {
	case <-p.ctx.Done():
		return Result{Err: p.ctx.Err()}
	case result := <-done:
		return result
	}
}

// Results returns the channel of results.
func (p *Pool) Results() <-chan Result {
	return p.resultCh
}

// Stop gracefully shuts down the pool.
// Waits for in-flight tasks to complete.
func (p *Pool) Stop() {
	p.mu.Lock()
	if !p.started {
		p.mu.Unlock()
		return
	}
	p.started = false
	p.mu.Unlock()

	close(p.taskCh)
	p.wg.Wait()
	close(p.resultCh)
}

// StopNow immediately cancels all workers.
func (p *Pool) StopNow() {
	p.cancel()
	close(p.taskCh)
	p.wg.Wait()
	close(p.resultCh)
}

// FanIn collects results from multiple pools into a single channel.
func FanIn(ctx context.Context, pools ...*Pool) <-chan Result {
	combined := make(chan Result)

	var wg sync.WaitGroup
	wg.Add(len(pools))

	for _, pool := range pools {
		go func(p *Pool) {
			defer wg.Done()
			for {
				select {
				case <-ctx.Done():
					return
				case result, ok := <-p.Results():
					if !ok {
						return
					}
					select {
					case <-ctx.Done():
						return
					case combined <- result:
					}
				}
			}
		}(pool)
	}

	go func() {
		wg.Wait()
		close(combined)
	}()

	return combined
}
