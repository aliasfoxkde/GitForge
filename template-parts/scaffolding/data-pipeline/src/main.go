// ETL Data Pipeline - Production-ready extract, transform, load workflow
package main

import (
	"context"
	"encoding/csv"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/redis/go-redis/v9"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

var (
	metricsRecordsProcessed = prometheus.NewCounterVec(
		prometheus.CounterOpts{
			Name: "etl_records_processed_total",
			Help: "Total number of records processed",
		},
		[]string{"stage", "status"},
	)
	metricsProcessingDuration = prometheus.NewHistogramVec(
		prometheus.HistogramOpts{
			Name:    "etl_processing_duration_seconds",
			Help:    "Time spent processing records",
			Buckets: prometheus.DefBuckets,
		},
		[]string{"stage"},
	)
	metricsBatchSize = prometheus.NewGauge(
		prometheus.GaugeOpts{
			Name: "etl_batch_size",
			Help: "Current batch size",
		},
	)
	metricsQueueDepth = prometheus.NewGauge(
		prometheus.GaugeOpts{
			Name: "etl_queue_depth",
			Help: "Current queue depth",
		},
	)
)

func init() {
	prometheus.MustRegister(metricsRecordsProcessed, metricsProcessingDuration, metricsBatchSize, metricsQueueDepth)
}

// Config holds pipeline configuration from environment variables
type Config struct {
	// Pipeline settings
	BatchSize       int
	Workers         int
	ShutdownTimeout time.Duration

	// Source settings
	SourceType     string // "csv", "http", "postgres"
	SourcePath     string
	SourceURL      string
	SourceInterval time.Duration

	// Destination settings
	DestType string // "postgres", "stdout"
	DestConn string // connection string or "" for stdout

	// Redis settings
	RedisAddr     string
	RedisPassword string
	RedisDB       int

	// Database settings
	DBHost     string
	DBPort     string
	DBUser     string
	DBPassword string
	DBName     string
	DBPoolSize int

	// HTTP server
	MetricsPort string
}

func loadConfig() Config {
	convInt := func(key, fallback string) int {
		if v := os.Getenv(key); v != "" {
			if i, err := strconv.Atoi(v); err == nil {
				return i
			}
		}
		i, _ := strconv.Atoi(fallback)
		return i
	}
	convDur := func(key, fallback string) time.Duration {
		if v := os.Getenv(key); v != "" {
			if d, err := time.ParseDuration(v); err == nil {
				return d
			}
		}
		d, _ := time.ParseDuration(fallback)
		return d
	}
	convIntDB := func(key, fallback string) int {
		return convInt(key, fallback)
	}

	return Config{
		BatchSize:       convInt("BATCH_SIZE", "100"),
		Workers:         convInt("WORKERS", "4"),
		ShutdownTimeout: convDur("SHUTDOWN_TIMEOUT", "30s"),
		SourceType:      getEnv("SOURCE_TYPE", "csv"),
		SourcePath:      getEnv("SOURCE_PATH", "/data/source.csv"),
		SourceURL:       getEnv("SOURCE_URL", "http://api.example.com/data"),
		SourceInterval:  convDur("SOURCE_INTERVAL", "1m"),
		DestType:        getEnv("DEST_TYPE", "postgres"),
		DestConn:        getEnv("DEST_CONN", ""),
		RedisAddr:       getEnv("REDIS_ADDR", "localhost:6379"),
		RedisPassword:   getEnv("REDIS_PASSWORD", ""),
		RedisDB:         convInt("REDIS_DB", "0"),
		DBHost:          getEnv("DB_HOST", "localhost"),
		DBPort:          getEnv("DB_PORT", "5432"),
		DBUser:          getEnv("DB_USER", "etl"),
		DBPassword:      getEnv("DB_PASSWORD", "etlsecret"),
		DBName:          getEnv("DB_NAME", "etl_db"),
		DBPoolSize:      convIntDB("DB_POOL_SIZE", "10"),
		MetricsPort:     getEnv("METRICS_PORT", "9090"),
	}
}

func getEnv(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

// Record represents a single data record in the pipeline
type Record map[string]interface{}

// Stage represents a processing stage in the ETL pipeline
type Stage string

const (
	StageExtract  Stage = "extract"
	StageTransform Stage = "transform"
	StageLoad     Stage = "load"
)

// Pipeline represents the ETL pipeline state
type Pipeline struct {
	cfg    Config
	logger *slog.Logger
	wg     sync.WaitGroup
	cancel context.CancelFunc

	// Channels for data flow
	recordChan chan Record
	errorChan  chan error
	doneChan   chan struct{}

	// External connections
	dbPool    *pgxpool.Pool
	redisClient *redis.Client

	// Metrics
	processedCount int64
	mu             sync.Mutex
}

func NewPipeline(cfg Config, logger *slog.Logger) *Pipeline {
	return &Pipeline{
		cfg:        cfg,
		logger:     logger,
		recordChan: make(chan Record, cfg.BatchSize*2),
		errorChan:  make(chan error, 100),
		doneChan:   make(chan struct{}),
	}
}

// Connect establishes connections to external services
func (p *Pipeline) Connect(ctx context.Context) error {
	var err error

	// Connect to PostgreSQL
	if p.cfg.DestType == "postgres" {
		connStr := fmt.Sprintf("postgres://%s:%s@%s:%s/%s?pool_max_conns=%d",
			p.cfg.DBUser, p.cfg.DBPassword, p.cfg.DBHost, p.cfg.DBPort, p.cfg.DBName, p.cfg.DBPoolSize)
		p.dbPool, err = pgxpool.New(ctx, connStr)
		if err != nil {
			return fmt.Errorf("failed to connect to postgres: %w", err)
		}
		if err := p.dbPool.Ping(ctx); err != nil {
			return fmt.Errorf("postgres ping failed: %w", err)
		}
		p.logger.Info("Connected to PostgreSQL", "host", p.cfg.DBHost, "database", p.cfg.DBName)
	}

	// Connect to Redis
	p.redisClient = redis.NewClient(&redis.Options{
		Addr:     p.cfg.RedisAddr,
		Password: p.cfg.RedisPassword,
		DB:       p.cfg.RedisDB,
	})
	if _, err := p.redisClient.Ping(ctx).Result(); err != nil {
		return fmt.Errorf("redis ping failed: %w", err)
	}
	p.logger.Info("Connected to Redis", "addr", p.cfg.RedisAddr)

	return nil
}

// Close closes all connections
func (p *Pipeline) Close() {
	if p.dbPool != nil {
		p.dbPool.Close()
	}
	if p.redisClient != nil {
		p.redisClient.Close()
	}
}

// Extract reads data from the source
func (p *Pipeline) Extract(ctx context.Context) error {
	p.logger.Info("Starting extraction", "type", p.cfg.SourceType)

	switch p.cfg.SourceType {
	case "csv":
		return p.extractFromCSV(ctx)
	case "http":
		return p.extractFromHTTP(ctx)
	case "postgres":
		return p.extractFromPostgres(ctx)
	default:
		return fmt.Errorf("unknown source type: %s", p.cfg.SourceType)
	}
}

func (p *Pipeline) extractFromCSV(ctx context.Context) error {
	file, err := os.Open(p.cfg.SourcePath)
	if err != nil {
		return fmt.Errorf("failed to open source file: %w", err)
	}
	defer file.Close()

	reader := csv.NewReader(file)
	headers, err := reader.Read()
	if err != nil {
		return fmt.Errorf("failed to read CSV headers: %w", err)
	}

	batch := make([]Record, 0, p.cfg.BatchSize)
	for {
		row, err := reader.Read()
		if err != nil {
			if len(batch) > 0 {
				p.sendBatch(ctx, batch)
			}
			if strings.Contains(err.Error(), "EOF") {
				break
			}
			return fmt.Errorf("failed to read CSV row: %w", err)
		}

		record := make(Record)
		for i, val := range row {
			if i < len(headers) {
				record[headers[i]] = val
			}
		}
		batch = append(batch, record)

		if len(batch) >= p.cfg.BatchSize {
			p.sendBatch(ctx, batch)
			batch = make([]Record, 0, p.cfg.BatchSize)
		}
	}

	return nil
}

func (p *Pipeline) extractFromHTTP(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, "GET", p.cfg.SourceURL, nil)
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}

	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("failed to fetch data: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("unexpected status code: %d", resp.StatusCode)
	}

	var records []Record
	if err := json.NewDecoder(resp.Body).Decode(&records); err != nil {
		return fmt.Errorf("failed to decode JSON: %w", err)
	}

	for i := 0; i < len(records); i += p.cfg.BatchSize {
		end := i + p.cfg.BatchSize
		if end > len(records) {
			end = len(records)
		}
		p.sendBatch(ctx, records[i:end])
	}

	return nil
}

func (p *Pipeline) extractFromPostgres(ctx context.Context) error {
	rows, err := p.dbPool.Query(ctx, "SELECT id, data, created_at FROM source_table")
	if err != nil {
		return fmt.Errorf("failed to query source: %w", err)
	}
	defer rows.Close()

	batch := make([]Record, 0, p.cfg.BatchSize)
	for rows.Next() {
		var id int
		var data string
		var createdAt time.Time
		if err := rows.Scan(&id, &data, &createdAt); err != nil {
			return fmt.Errorf("failed to scan row: %w", err)
		}
		record := Record{"id": id, "data": data, "created_at": createdAt}
		batch = append(batch, record)

		if len(batch) >= p.cfg.BatchSize {
			p.sendBatch(ctx, batch)
			batch = make([]Record, 0, p.cfg.BatchSize)
		}
	}

	if len(batch) > 0 {
		p.sendBatch(ctx, batch)
	}

	return rows.Err()
}

func (p *Pipeline) sendBatch(ctx context.Context, batch []Record) {
	metricsBatchSize.Set(float64(len(batch)))
	metricsQueueDepth.Set(float64(len(p.recordChan)))

	for _, record := range batch {
		select {
		case p.recordChan <- record:
		case <-ctx.Done():
			return
		}
	}
}

// Transform applies transformations to records
func (p *Pipeline) Transform(ctx context.Context) error {
	p.logger.Info("Starting transformation", "workers", p.cfg.Workers)

	var wg sync.WaitGroup
	errChan := make(chan error, p.cfg.Workers)

	for i := 0; i < p.cfg.Workers; i++ {
		wg.Add(1)
		go func(workerID int) {
			defer wg.Done()
			p.transformWorker(ctx, workerID)
		}(i)
	}

	wg.Wait()
	close(p.recordChan)

	select {
	case err := <-errChan:
		return err
	default:
		return nil
	}
}

func (p *Pipeline) transformWorker(ctx context.Context, workerID int) {
	for record := range p.recordChan {
		start := time.Now()

		// Apply transformations
		record = p.normalizeRecord(record)
		record = p.enrichRecord(record)
		record = p.validateRecord(record)

		// Forward to load stage
		select {
		case p.recordChan <- record:
		case <-ctx.Done():
			return
		}

		metricsProcessingDuration.WithLabelValues(string(StageTransform)).Observe(time.Since(start).Seconds())
		metricsRecordsProcessed.WithLabelValues(string(StageTransform), "success").Inc()
	}
}

func (p *Pipeline) normalizeRecord(record Record) Record {
	// String trimming and normalization
	for k, v := range record {
		if s, ok := v.(string); ok {
			record[k] = strings.TrimSpace(s)
		}
	}
	return record
}

func (p *Pipeline) enrichRecord(record Record) Record {
	// Add metadata
	record["processed_at"] = time.Now().UTC().Format(time.RFC3339)
	record["pipeline_version"] = "1.0.0"

	// Calculate derived fields if applicable
	if val, ok := record["amount"]; ok {
		if strVal, ok := val.(string); ok {
			if amount, err := strconv.ParseFloat(strVal, 64); err == nil {
				record["amount_numeric"] = amount
				record["amount_currency"] = "USD"
			}
		}
	}

	return record
}

func (p *Pipeline) validateRecord(record Record) Record {
	// Ensure required fields exist
	required := []string{"id", "processed_at"}
	for _, field := range required {
		if _, ok := record[field]; !ok {
			record[field] = nil
		}
	}
	return record
}

// Load writes records to the destination
func (p *Pipeline) Load(ctx context.Context) error {
	p.logger.Info("Starting load", "type", p.cfg.DestType)

	switch p.cfg.DestType {
	case "postgres":
		return p.loadToPostgres(ctx)
	case "stdout":
		return p.loadToStdout(ctx)
	default:
		return fmt.Errorf("unknown destination type: %s", p.cfg.DestType)
	}
}

func (p *Pipeline) loadToPostgres(ctx context.Context) error {
	// Ensure table exists
	_, err := p.dbPool.Exec(ctx, `
		CREATE TABLE IF NOT EXISTS etl_records (
			id SERIAL PRIMARY KEY,
			data JSONB NOT NULL,
			processed_at TIMESTAMP NOT NULL,
			created_at TIMESTAMP DEFAULT NOW()
		)
	`)
	if err != nil {
		return fmt.Errorf("failed to create table: %w", err)
	}

	batch := &pgx.Batch{}
	count := 0

	for record := range p.recordChan {
		data, err := json.Marshal(record)
		if err != nil {
			p.logger.Error("Failed to marshal record", "error", err)
			metricsRecordsProcessed.WithLabelValues(string(StageLoad), "error").Inc()
			continue
		}

		batch.Queue("INSERT INTO etl_records (data, processed_at) VALUES ($1, $2)",
			data, record["processed_at"])

		count++
		if count >= p.cfg.BatchSize {
			br := p.dbPool.SendBatch(ctx, batch)
			for i := 0; i < batch.Len(); i++ {
				if _, err := br.Exec(); err != nil {
					metricsRecordsProcessed.WithLabelValues(string(StageLoad), "error").Inc()
				} else {
					metricsRecordsProcessed.WithLabelValues(string(StageLoad), "success").Inc()
				}
			}
			br.Close()
			batch = &pgx.Batch{}
			count = 0
		}
	}

	// Flush remaining
	if count > 0 {
		br := p.dbPool.SendBatch(ctx, batch)
		for i := 0; i < batch.Len(); i++ {
			if _, err := br.Exec(); err != nil {
				metricsRecordsProcessed.WithLabelValues(string(StageLoad), "error").Inc()
			} else {
				metricsRecordsProcessed.WithLabelValues(string(StageLoad), "success").Inc()
			}
		}
		br.Close()
	}

	p.logger.Info("Load completed", "records", count)
	return nil
}

func (p *Pipeline) loadToStdout(ctx context.Context) error {
	encoder := json.NewEncoder(os.Stdout)
	for record := range p.recordChan {
		if err := encoder.Encode(record); err != nil {
			p.logger.Error("Failed to write record", "error", err)
			continue
		}
		metricsRecordsProcessed.WithLabelValues(string(StageLoad), "success").Inc()
	}
	return nil
}

// Run executes the full ETL pipeline
func (p *Pipeline) Run(ctx context.Context) error {
	ctx, p.cancel = context.WithCancel(ctx)
	defer p.cancel()

	// Start metrics server
	go p.startMetricsServer()

	// Connect to services
	if err := p.Connect(ctx); err != nil {
		return fmt.Errorf("connection failed: %w", err)
	}
	defer p.Close()

	p.logger.Info("Pipeline starting",
		"batch_size", p.cfg.BatchSize,
		"workers", p.cfg.Workers,
		"source", p.cfg.SourceType,
		"destination", p.cfg.DestType,
	)

	// Extract
	if err := p.Extract(ctx); err != nil {
		return fmt.Errorf("extract failed: %w", err)
	}
	close(p.recordChan)

	// Transform
	if err := p.Transform(ctx); err != nil {
		return fmt.Errorf("transform failed: %w", err)
	}

	// Load
	if err := p.Load(ctx); err != nil {
		return fmt.Errorf("load failed: %w", err)
	}

	p.logger.Info("Pipeline completed successfully")
	return nil
}

func (p *Pipeline) startMetricsServer() {
	mux := http.NewServeMux()
	mux.Handle("/metrics", promhttp.Handler())
	mux.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]string{"status": "healthy"})
	})
	mux.HandleFunc("/ready", func(w http.ResponseWriter, r *http.Request) {
		if p.dbPool != nil && p.redisClient != nil {
			w.WriteHeader(http.StatusOK)
		} else {
			w.WriteHeader(http.StatusServiceUnavailable)
		}
	})

	addr := ":" + p.cfg.MetricsPort
	p.logger.Info("Metrics server starting", "addr", addr)
	if err := http.ListenAndServe(addr, mux); err != nil && err != http.ErrServerClosed {
		p.logger.Error("Metrics server error", "error", err)
	}
}

// SetupSignalHandling returns a context that cancels on SIGINT/SIGTERM
func SetupSignalHandling() (context.Context, context.CancelFunc) {
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		sigChan := make(chan os.Signal, 1)
		signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)
		<-sigChan
		cancel()
	}()
	return ctx, cancel
}

func main() {
	// Setup structured logging
	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	}))
	slog.SetDefault(logger)

	cfg := loadConfig()

	ctx, cancel := SetupSignalHandling()
	defer cancel()

	pipeline := NewPipeline(cfg, logger)

	// Run with graceful shutdown
	done := make(chan error, 1)
	go func() {
		done <- pipeline.Run(ctx)
	}()

	select {
	case err := <-done:
		if err != nil {
			logger.Error("Pipeline failed", "error", err)
			os.Exit(1)
		}
	case <-ctx.Done():
		logger.Info("Shutdown signal received, stopping pipeline...")
		cancel()
		<-done
	}
}