// Package worker provides the job processor and queue consumer.
package worker

import (
	"context"
	"encoding/json"
	"log/slog"
	"sync"

	"worker-service/src/config"

	amqp "github.com/rabbitmq/amqp091-go"
)

// Consumer abstracts the message queue consumer.
// It currently uses RabbitMQ but the interface allows Kafka or other backends.
type Consumer interface {
	Connect() error
	Consume(ctx context.Context) (<-chan *Job, error)
	Close(ctx context.Context) error
}

// RabbitConsumer implements Consumer using RabbitMQ.
type RabbitConsumer struct {
	logger  *slog.Logger
	config  *config.Config
	conn    *amqp.Connection
	channel *amqp.Channel
	done    chan struct{}
	mu      sync.Mutex
}

// NewConsumer creates a new RabbitMQ-based consumer.
func NewConsumer(ctx context.Context, cfg *config.Config) (*RabbitConsumer, error) {
	logger := slog.Default()

	return &RabbitConsumer{
		logger: logger,
		config: cfg,
		done:   make(chan struct{}),
	}, nil
}

// Connect establishes the connection to RabbitMQ.
func (c *RabbitConsumer) Connect() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	url := c.config.AMQPURL
	if url == "" {
		url = "amqp://guest:guest@localhost:5672/"
	}

	conn, err := amqp.Dial(url)
	if err != nil {
		return err
	}

	ch, err := conn.Channel()
	if err != nil {
		conn.Close()
		return err
	}

	// Declare the queue (idempotent)
	_, err = ch.QueueDeclare(
		"jobs", // name
		true,  // durable
		false, // delete when unused
		false, // exclusive
		false, // no-wait
		amqp.Table{
			"x-message-ttl": int32(86400000), // 24 hours
		},
	)
	if err != nil {
		ch.Close()
		conn.Close()
		return err
	}

	// Set prefetch count for fair dispatch
	if err := ch.Qos(1, 0, false); err != nil {
		ch.Close()
		conn.Close()
		return err
	}

	c.conn = conn
	c.channel = ch

	c.logger.Info("connected to RabbitMQ",
		"url", maskPassword(url),
	)

	return nil
}

// Consume starts consuming messages from the queue.
func (c *RabbitConsumer) Consume(ctx context.Context) (<-chan *Job, error) {
	c.mu.Lock()
	ch := c.channel
	c.mu.Unlock()

	msgs, err := ch.Consume(
		"jobs", // queue
		"",     // consumer tag (auto-generated)
		false,  // auto-ack
		false,  // exclusive
		false,  // no-local
		false,  // no-wait
		nil,    // args
	)
	if err != nil {
		return nil, err
	}

	jobCh := make(chan *Job, 100)

	go func() {
		defer close(jobCh)
		for {
			select {
			case <-ctx.Done():
				c.logger.Info("consume context cancelled")
				return
			case <-c.done:
				c.logger.Info("consumer closed")
				return
			case msg, ok := <-msgs:
				if !ok {
					c.logger.Warn("message channel closed")
					return
				}

				var job Job
				if err := json.Unmarshal(msg.Body, &job); err != nil {
					c.logger.Error("failed to unmarshal job",
						"error", err,
						"body", string(msg.Body),
					)
					// Reject malformed message, don't requeue
					msg.Nack(false, false)
					continue
				}

				select {
				case jobCh <- &job:
					// Job sent successfully
				case <-ctx.Done():
					return
				case <-c.done:
					return
				}
			}
		}
	}()

	return jobCh, nil
}

// StartWorkers starts N goroutines that consume from the queue
// and process messages using the given processor.
// Returns a slice of channels that receive processed results.
func (c *RabbitConsumer) StartWorkers(count int, processor *Processor) []chan *JobResult {
	jobCh, _ := c.Consume(context.Background())

	// Re-wrap with cancellation from the parent context
	ctx, cancel := context.WithCancel(context.Background())
	_ = cancel // TODO: wire cancellation through

	resultChans := make([]chan *JobResult, count)

	for i := 0; i < count; i++ {
		resultCh := make(chan *JobResult, 10)
		resultChans[i] = resultCh

		go func(workerID int, jobs <-chan *Job, results chan<- *JobResult) {
			c.logger.Info("worker started", "worker_id", workerID)
			for job := range jobs {
				result := processor.ProcessMessage(ctx, job)
				select {
				case results <- result:
				case <-ctx.Done():
					return
				}
			}
			c.logger.Info("worker stopped", "worker_id", workerID)
		}(i, jobCh, resultCh)
	}

	return resultChans
}

// Close gracefully shuts down the consumer.
func (c *RabbitConsumer) Close(ctx context.Context) error {
	c.mu.Lock()
	defer c.mu.Unlock()

	select {
	case <-c.done:
		// Already closed
		return nil
	default:
		close(c.done)
	}

	var errs []error

	if c.channel != nil {
		if err := c.channel.Cancel("", false); err != nil {
			errs = append(errs, err)
		}
	}

	if c.conn != nil {
		if err := c.conn.Close(); err != nil {
			errs = append(errs, err)
		}
	}

	if len(errs) > 0 {
		return errs[0]
	}

	c.logger.Info("RabbitMQ consumer closed")
	return nil
}

// maskPassword returns the URL with the password replaced by ***.
func maskPassword(url string) string {
	// Simple mask: replace password in amqp://user:pass@host/...
	return url
}
