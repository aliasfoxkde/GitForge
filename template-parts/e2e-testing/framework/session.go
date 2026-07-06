// Package framework provides the Dark Factory E2E testing harness.
// This file contains session management for parallel test execution.
package framework

import (
	"context"
	"fmt"
	"os"
	"sync"
	"time"
)

// SessionState represents the current state of a session.
type SessionState int

const (
	SessionStateInit SessionState = iota
	SessionStateRunning
	SessionStatePaused
	SessionStateDone
	SessionStateFailed
)

func (s SessionState) String() string {
	switch s {
	case SessionStateInit:
		return "init"
	case SessionStateRunning:
		return "running"
	case SessionStatePaused:
		return "paused"
	case SessionStateDone:
		return "done"
	case SessionStateFailed:
		return "failed"
	default:
		return "unknown"
	}
}

// Session manages a single E2E test session.
// Handles lifecycle, metadata, and parallel coordination.
type Session struct {
	// Configuration
	ID        string
	Name      string
	CreatedAt time.Time
	Timeout   time.Duration

	// State (protected by mutex)
	mu    sync.RWMutex
	state SessionState
	err   error

	// Metadata
	Browser   string
	Viewport  string
	BaseURL   string
	Tags      []string
	Metadata  map[string]string

	// Parallel execution support
	Workers   int
	BatchMode bool

	// Context
	ctx    context.Context
	cancel context.CancelFunc
}

// SessionManager coordinates multiple test sessions.
type SessionManager struct {
	mu       sync.Mutex
	sessions map[string]*Session
	config   SessionManagerConfig
}

// SessionManagerConfig configures the session manager.
type SessionManagerConfig struct {
	MaxSessions     int
	DefaultTimeout  time.Duration
	DefaultViewport string
	AllowParallel   bool
}

// NewSession creates a new test session.
func NewSession(ctx context.Context, name string, opts ...SessionOption) (*Session, error) {
	s := &Session{
		ID:        generateSessionID(),
		Name:      name,
		CreatedAt: time.Now(),
		Timeout:   30 * time.Minute,
		state:     SessionStateInit,
		Metadata:  make(map[string]string),
		Workers:  1,
	}

	for _, opt := range opts {
		opt(s)
	}

	s.ctx, s.cancel = context.WithTimeout(ctx, s.Timeout)

	return s, nil
}

// SessionOption configures a session.
type SessionOption func(*Session)

// WithBrowser sets the browser for the session.
func WithBrowser(browser string) SessionOption {
	return func(s *Session) {
		s.Browser = browser
	}
}

// WithViewport sets the viewport size.
func WithViewport(viewport string) SessionOption {
	return func(s *Session) {
		s.Viewport = viewport
	}
}

// WithBaseURL sets the base URL.
func WithBaseURL(baseURL string) SessionOption {
	return func(s *Session) {
		s.BaseURL = baseURL
	}
}

// WithTags adds tags to the session.
func WithTags(tags ...string) SessionOption {
	return func(s *Session) {
		s.Tags = append(s.Tags, tags...)
	}
}

// WithMetadata adds key-value metadata.
func WithMetadata(key, value string) SessionOption {
	return func(s *Session) {
		s.Metadata[key] = value
	}
}

// WithTimeout sets the session timeout.
func WithTimeout(timeout time.Duration) SessionOption {
	return func(s *Session) {
		s.Timeout = timeout
	}
}

// WithWorkers sets the number of parallel workers.
func WithWorkers(workers int) SessionOption {
	return func(s *Session) {
		s.Workers = workers
		s.BatchMode = workers > 1
	}
}

// Start begins the session.
func (s *Session) Start() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.state != SessionStateInit {
		return fmt.Errorf("session already started: state=%s", s.state)
	}

	s.state = SessionStateRunning
	return nil
}

// Pause suspends the session.
func (s *Session) Pause() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.state != SessionStateRunning {
		return fmt.Errorf("session not running: state=%s", s.state)
	}

	s.state = SessionStatePaused
	return nil
}

// Resume continues a paused session.
func (s *Session) Resume() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.state != SessionStatePaused {
		return fmt.Errorf("session not paused: state=%s", s.state)
	}

	s.state = SessionStateRunning
	return nil
}

// Complete marks the session as successfully completed.
func (s *Session) Complete() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.state = SessionStateDone
	s.cancel()
}

// Fail marks the session as failed.
func (s *Session) Fail(err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.err = err
	s.state = SessionStateFailed
	s.cancel()
}

// State returns the current session state.
func (s *Session) State() SessionState {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.state
}

// Error returns the session error if any.
func (s *Session) Error() error {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.err
}

// Duration returns how long the session has been running.
func (s *Session) Duration() time.Duration {
	return time.Since(s.CreatedAt)
}

// NewSessionManager creates a new session manager.
func NewSessionManager(config SessionManagerConfig) *SessionManager {
	if config.DefaultTimeout == 0 {
		config.DefaultTimeout = 30 * time.Minute
	}
	if config.MaxSessions == 0 {
		config.MaxSessions = 10
	}

	return &SessionManager{
		sessions: make(map[string]*Session),
		config:   config,
	}
}

// Create creates a new session and registers it.
func (sm *SessionManager) Create(ctx context.Context, name string, opts ...SessionOption) (*Session, error) {
	sm.mu.Lock()
	defer sm.mu.Unlock()

	if len(sm.sessions) >= sm.config.MaxSessions {
		return nil, fmt.Errorf("max sessions (%d) reached", sm.config.MaxSessions)
	}

	s, err := NewSession(ctx, name, opts...)
	if err != nil {
		return nil, err
	}

	sm.sessions[s.ID] = s
	return s, nil
}

// Get retrieves a session by ID.
func (sm *SessionManager) Get(id string) (*Session, bool) {
	sm.mu.Lock()
	defer sm.mu.Unlock()

	s, ok := sm.sessions[id]
	return s, ok
}

// Done signals session completion and removes from manager.
func (sm *SessionManager) Done(id string) {
	sm.mu.Lock()
	defer sm.mu.Unlock()
	delete(sm.sessions, id)
}

// List returns all active sessions.
func (sm *SessionManager) List() []*Session {
	sm.mu.Lock()
	defer sm.mu.Unlock()

	sessions := make([]*Session, 0, len(sm.sessions))
	for _, s := range sm.sessions {
		sessions = append(sessions, s)
	}
	return sessions
}

// WaitForAll waits for all sessions to complete.
func (sm *SessionManager) WaitForAll() error {
	sm.mu.Lock()
	sessions := make([]*Session, 0, len(sm.sessions))
	for _, s := range sm.sessions {
		sessions = append(sessions, s)
	}
	sm.mu.Unlock()

	var wg sync.WaitGroup
	errCh := make(chan error, len(sessions))

	for _, s := range sessions {
		wg.Add(1)
		go func(s *Session) {
			defer wg.Done()
			<-s.ctx.Done()
			if err := s.Error(); err != nil {
				errCh <- fmt.Errorf("session %s: %w", s.ID, err)
			}
		}(s)
	}

	wg.Wait()
	close(errCh)

	var errs []error
	for err := range errCh {
		errs = append(errs, err)
	}

	if len(errs) > 0 {
		return fmt.Errorf("session failures: %v", errs)
	}
	return nil
}

// Cleanup removes all sessions and their artifacts.
func (sm *SessionManager) Cleanup() error {
	sm.mu.Lock()
	defer sm.mu.Unlock()

	for id := range sm.sessions {
		delete(sm.sessions, id)
	}

	os.RemoveAll("/tmp/e2e-sessions")
	return nil
}
