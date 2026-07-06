// Package framework provides the Dark Factory E2E testing harness.
// This file contains debugging utilities for test failure investigation.
package framework

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// DebugConfig configures debugging behavior.
type DebugConfig struct {
	Enabled bool

	ScreenshotPath string
	CaptureConsole bool
	ConsoleLogPath string
	CaptureNetwork bool
	NetworkLogPath string
	DumpStateOnFailure bool
	StateDumpPath      string
	DebugServer bool
	DebugPort   int
}

// Debugger provides debugging utilities for E2E tests.
type Debugger struct {
	config     DebugConfig
	sessionID  string

	consoleLogs  []ConsoleLog
	networkLogs  []NetworkLog
	screenshots  []Screenshot

	ctx    context.Context
	cancel context.CancelFunc
}

// ConsoleLog represents a captured console message.
type ConsoleLog struct {
	Type      string    `json:"type"`
	Message   string    `json:"message"`
	Timestamp time.Time `json:"timestamp"`
	Source    string    `json:"source"`
}

// NetworkLog represents a captured network request/response.
type NetworkLog struct {
	Method         string            `json:"method"`
	URL            string            `json:"url"`
	Status         int               `json:"status"`
	RequestHeaders  map[string]string `json:"request_headers"`
	ResponseHeaders map[string]string `json:"response_headers"`
	Body           string            `json:"body,omitempty"`
	Duration       time.Duration     `json:"duration"`
	Timestamp      time.Time         `json:"timestamp"`
}

// Screenshot represents a captured screenshot.
type Screenshot struct {
	Name      string    `json:"name"`
	Path      string    `json:"path"`
	Timestamp time.Time `json:"timestamp"`
	Viewport  string    `json:"viewport"`
	URL       string    `json:"url"`
}

// StateDump represents a captured application state.
type StateDump struct {
	SessionID  string                 `json:"session_id"`
	Timestamp  time.Time              `json:"timestamp"`
	URL        string                 `json:"url"`
	LocalStorage   map[string]interface{} `json:"local_storage,omitempty"`
	SessionStorage map[string]interface{} `json:"session_storage,omitempty"`
	Cookies    []Cookie               `json:"cookies,omitempty"`
	CustomData map[string]interface{}  `json:"custom_data,omitempty"`
}

// Cookie represents a browser cookie.
type Cookie struct {
	Name     string    `json:"name"`
	Value    string    `json:"value"`
	Domain   string    `json:"domain"`
	Path     string    `json:"path"`
	Expires  time.Time `json:"expires,omitempty"`
	Secure   bool      `json:"secure"`
	HTTPOnly bool      `json:"http_only"`
}

// NewDebugger creates a new debugger instance.
func NewDebugger(sessionID string, config DebugConfig) *Debugger {
	ctx, cancel := context.WithCancel(context.Background())

	d := &Debugger{
		config:     config,
		sessionID:  sessionID,
		consoleLogs: make([]ConsoleLog, 0),
		networkLogs: make([]NetworkLog, 0),
		screenshots: make([]Screenshot, 0),
		ctx:         ctx,
	}

	if config.Enabled {
		d.ensureDirectories()
	}

	return d
}

func (d *Debugger) ensureDirectories() {
	dirs := []string{
		d.config.ScreenshotPath,
		d.config.ConsoleLogPath,
		d.config.NetworkLogPath,
		d.config.StateDumpPath,
	}

	for _, dir := range dirs {
		if dir != "" {
			os.MkdirAll(dir, 0755)
		}
	}

	if d.config.ScreenshotPath == "" {
		d.config.ScreenshotPath = "/tmp/e2e-debug/screenshots"
	}
	if d.config.ConsoleLogPath == "" {
		d.config.ConsoleLogPath = "/tmp/e2e-debug/console"
	}
	if d.config.NetworkLogPath == "" {
		d.config.NetworkLogPath = "/tmp/e2e-debug/network"
	}
	if d.config.StateDumpPath == "" {
		d.config.StateDumpPath = "/tmp/e2e-debug/state"
	}

	for _, dir := range []string{
		d.config.ScreenshotPath,
		d.config.ConsoleLogPath,
		d.config.NetworkLogPath,
		d.config.StateDumpPath,
	} {
		os.MkdirAll(dir, 0755)
	}
}

// CaptureScreenshot captures a screenshot.
func (d *Debugger) CaptureScreenshot(name, url, viewport string) (*Screenshot, error) {
	if !d.config.Enabled {
		return nil, nil
	}

	filename := fmt.Sprintf("%s-%s-%s.png",
		d.sessionID,
		name,
		time.Now().Format("20060102-150405"))
	path := filepath.Join(d.config.ScreenshotPath, filename)

	s := &Screenshot{
		Name:      name,
		Path:      path,
		Timestamp: time.Now(),
		Viewport:  viewport,
		URL:       url,
	}

	d.screenshots = append(d.screenshots, *s)

	fmt.Fprintf(os.Stderr, "[debugger] Screenshot: %s -> %s\n", name, path)

	return s, nil
}

// LogConsole records a console message.
func (d *Debugger) LogConsole(msgType, message, source string) {
	if !d.config.Enabled || !d.config.CaptureConsole {
		return
	}

	log := ConsoleLog{
		Type:      msgType,
		Message:   message,
		Timestamp: time.Now(),
		Source:    source,
	}

	d.consoleLogs = append(d.consoleLogs, log)
}

// LogNetwork records a network request/response.
func (d *Debugger) LogNetwork(method, url string, status int, reqHeaders, respHeaders map[string]string, body string, duration time.Duration) {
	if !d.config.Enabled || !d.config.CaptureNetwork {
		return
	}

	log := NetworkLog{
		Method:          method,
		URL:             url,
		Status:          status,
		RequestHeaders:  reqHeaders,
		ResponseHeaders: respHeaders,
		Body:            body,
		Duration:        duration,
		Timestamp:       time.Now(),
	}

	d.networkLogs = append(d.networkLogs, log)
}

// DumpState captures the current application state.
func (d *Debugger) DumpState(url string) (*StateDump, error) {
	if !d.config.Enabled || !d.config.DumpStateOnFailure {
		return nil, nil
	}

	filename := fmt.Sprintf("state-%s-%s.json",
		d.sessionID,
		time.Now().Format("20060102-150405"))
	path := filepath.Join(d.config.StateDumpPath, filename)

	state := &StateDump{
		SessionID:  d.sessionID,
		Timestamp:  time.Now(),
		URL:        url,
		CustomData: make(map[string]interface{}),
	}

	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return nil, fmt.Errorf("marshal state: %w", err)
	}

	if err := os.WriteFile(path, data, 0644); err != nil {
		return nil, fmt.Errorf("write state: %w", err)
	}

	fmt.Fprintf(os.Stderr, "[debugger] State dumped: %s\n", path)
	return state, nil
}

// OnFailure is called when a test fails, capturing all debug artifacts.
func (d *Debugger) OnFailure(testName, url string) error {
	if !d.config.Enabled {
		return nil
	}

	if _, err := d.CaptureScreenshot(testName, url, ""); err != nil {
		fmt.Fprintf(os.Stderr, "[debugger] Screenshot failed: %v\n", err)
	}

	if _, err := d.DumpState(url); err != nil {
		fmt.Fprintf(os.Stderr, "[debugger] State dump failed: %v\n", err)
	}

	return nil
}

// Flush writes all captured debug data to disk.
func (d *Debugger) Flush() error {
	if !d.config.Enabled {
		return nil
	}

	if d.config.CaptureConsole && len(d.consoleLogs) > 0 {
		path := filepath.Join(d.config.ConsoleLogPath, fmt.Sprintf("console-%s.json", d.sessionID))
		data, err := json.MarshalIndent(d.consoleLogs, "", "  ")
		if err != nil {
			return fmt.Errorf("marshal console logs: %w", err)
		}
		if err := os.WriteFile(path, data, 0644); err != nil {
			return fmt.Errorf("write console logs: %w", err)
		}
	}

	if d.config.CaptureNetwork && len(d.networkLogs) > 0 {
		path := filepath.Join(d.config.NetworkLogPath, fmt.Sprintf("network-%s.json", d.sessionID))
		data, err := json.MarshalIndent(d.networkLogs, "", "  ")
		if err != nil {
			return fmt.Errorf("marshal network logs: %w", err)
		}
		if err := os.WriteFile(path, data, 0644); err != nil {
			return fmt.Errorf("write network logs: %w", err)
		}
	}

	fmt.Fprintf(os.Stderr, "[debugger] Debug data flushed to /tmp/e2e-debug/\n")
	return nil
}

// Close cleans up debugger resources.
func (d *Debugger) Close() error {
	d.cancel()
	return d.Flush()
}

// NetworkRequest records a network request for later analysis.
type NetworkRequest struct {
	ID       string
	Method   string
	URL      string
	Body     []byte
	Headers  map[string]string
	Response *NetworkResponse
}

// NetworkResponse records a network response.
type NetworkResponse struct {
	Status     int
	StatusText string
	Headers    map[string]string
	Body       []byte
	Duration   time.Duration
}

// RecordRequestResponse pairs a request and response together.
func (d *Debugger) RecordRequestResponse(req *NetworkRequest, resp *NetworkResponse) {
	if !d.config.Enabled || !d.config.CaptureNetwork {
		return
	}

	d.networkLogs = append(d.networkLogs, NetworkLog{
		Method:          req.Method,
		URL:             req.URL,
		Status:          resp.Status,
		RequestHeaders:  req.Headers,
		ResponseHeaders: resp.Headers,
		Body:            string(resp.Body),
		Duration:        resp.Duration,
		Timestamp:       time.Now(),
	})
}
