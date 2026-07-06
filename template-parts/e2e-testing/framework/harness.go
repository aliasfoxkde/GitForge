// Package framework provides the Dark Factory E2E testing harness.
// Designed for 90%+ automated coverage through AI-assisted test generation.
package framework

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// CoverageTarget defines minimum coverage requirements.
type CoverageTarget struct {
	Branch    float64 // e.g., 0.80 for 80%
	Function  float64
	Line      float64
	Path      float64
}

// HarnessConfig configures the test harness.
type HarnessConfig struct {
	// Test configuration
	Timeout       time.Duration
	RetryCount    int
	Parallelism   int

	// Coverage targets
	CoverageTarget CoverageTarget

	// AI configuration
	AIEnabled     bool
	AIModel       string
	AICoverageMode bool  // Analyze uncovered code with AI

	// Reporter configuration
	ReportPath    string
	ReportFormat  string // "json", "html", "sarif"

	// Debug
	Verbose       bool
	DebugPort     int
}

// Harness is the main E2E testing harness.
// Provides setup, teardown, coverage tracking, and AI-assisted coverage analysis.
type Harness struct {
	config HarnessConfig
	ctx    context.Context
	cancel context.CancelFunc

	// State
	sessionID   string
	startTime   time.Time
	coverage    *CoverageReport
	testResults []TestResult

	// AI components
	coverageAnalyzer *AICoverageAnalyzer
}

// CoverageReport holds coverage data.
type CoverageReport struct {
	Branch  float64 `json:"branch"`
	Function float64 `json:"function"`
	Line    float64 `json:"line"`
	Path    float64 `json:"path"`

	Uncovered []UncoveredBlock `json:"uncovered,omitempty"`

	GeneratedAt time.Time `json:"generated_at"`
}

// UncoveredBlock represents a code block not covered by tests.
type UncoveredBlock struct {
	File     string `json:"file"`
	Line     int    `json:"line"`
	Function string `json:"function"`
	Reason   string `json:"reason"`
}

// TestResult holds the result of a single test.
type TestResult struct {
	Name      string        `json:"name"`
	Status    string        `json:"status"` // pass, fail, skip
	Duration  time.Duration `json:"duration"`
	Coverage  *CoverageReport `json:"coverage,omitempty"`
	Errors    []string      `json:"errors,omitempty"`
	DebugInfo string        `json:"debug_info,omitempty"`
}

// NewHarness creates and initializes a new test harness.
func NewHarness(ctx context.Context, config HarnessConfig) (*Harness, error) {
	if config.Timeout == 0 {
		config.Timeout = 30 * time.Minute
	}
	if config.RetryCount == 0 {
		config.RetryCount = 2
	}
	if config.Parallelism == 0 {
		config.Parallelism = 4
	}

	ctx, cancel := context.WithTimeout(ctx, config.Timeout)

	h := &Harness{
		config:     config,
		ctx:        ctx,
		cancel:     cancel,
		sessionID:  generateSessionID(),
		startTime:  time.Now(),
		testResults: make([]TestResult, 0),
	}

	if config.AIEnabled {
		h.coverageAnalyzer = NewAICoverageAnalyzer(config.AIModel, config.AICoverageMode)
	}

	return h, nil
}

// Setup prepares the harness for testing.
func (h *Harness) Setup() error {
	if h.config.Verbose {
		fmt.Fprintf(os.Stderr, "[harness] Setting up session %s\n", h.sessionID)
	}

	// Ensure report directory exists
	if h.config.ReportPath != "" {
		dir := filepath.Dir(h.config.ReportPath)
		if err := os.MkdirAll(dir, 0755); err != nil {
			return fmt.Errorf("create report dir: %w", err)
		}
	}

	// Initialize coverage tracking
	h.coverage = &CoverageReport{GeneratedAt: time.Now()}

	return nil
}

// Run executes a test suite with coverage tracking.
func (h *Harness) Run(name string, fn func(*TestingT)) error {
	if h.config.Verbose {
		fmt.Fprintf(os.Stderr, "[harness] Running test: %s\n", name)
	}

	start := time.Now()
	result := TestResult{Name: name}

	err := fn(&TestingT{
		harness: h,
		name:    name,
	})

	result.Duration = time.Since(start)

	if err != nil {
		result.Status = "fail"
		result.Errors = []string{err.Error()}
	} else {
		result.Status = "pass"
	}

	h.testResults = append(h.testResults, result)
	return err
}

// Teardown cleans up after testing and generates reports.
func (h *Harness) Teardown() error {
	if h.config.Verbose {
		fmt.Fprintf(os.Stderr, "[harness] Tearing down session %s\n", h.sessionID)
	}

	h.cancel()

	// Generate coverage report
	if err := h.generateCoverageReport(); err != nil {
		fmt.Fprintf(os.Stderr, "[harness] Warning: coverage report failed: %v\n", err)
	}

	// Write test results
	if err := h.writeResults(); err != nil {
		fmt.Fprintf(os.Stderr, "[harness] Warning: results write failed: %v\n", err)
	}

	return nil
}

func (h *Harness) generateCoverageReport() error {
	// Check coverage targets
	if h.config.AIEnabled && h.coverageAnalyzer != nil {
		return h.coverageAnalyzer.Analyze(h.coverage, h.config.CoverageTarget)
	}
	return nil
}

func (h *Harness) writeResults() error {
	if h.config.ReportPath == "" {
		return nil
	}

	data, err := json.MarshalIndent(h.testResults, "", "  ")
	if err != nil {
		return err
	}

	return os.WriteFile(h.config.ReportPath, data, 0644)
}

// TestingT is the testing interface provided to test functions.
type TestingT struct {
	harness *Harness
	name    string
}

func (t *TestingT) Error(args ...any) {
	fmt.Fprintln(os.Stderr, args...)
}

func (t *TestingT) Fatal(args ...any) {
	fmt.Fprintln(os.Stderr, args...)
	os.Exit(1)
}

func generateSessionID() string {
	return fmt.Sprintf("e2e-%d", time.Now().UnixNano())
}
