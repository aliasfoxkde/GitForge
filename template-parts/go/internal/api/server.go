// Package api provides the HTTP server implementation.
package api

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"net/http"
	"time"
)

// Server represents the HTTP server.
type Server struct {
	httpServer *http.Server
	logger     Logger
}

// Logger interface for structured logging.
type Logger interface {
	Info(msg string, args ...any)
	Error(msg string, args ...any)
}

// Config holds server configuration.
type Config struct {
	Addr         string
	ReadTimeout  time.Duration
	WriteTimeout time.Duration
	IdleTimeout  time.Duration
	TLS          *TLSConfig
}

// TLSConfig holds TLS configuration.
type TLSConfig struct {
	CertFile string
	KeyFile  string
}

// NewServer creates a new HTTP server.
func NewServer(config *Config, logger Logger) (*Server, error) {
	if config.Addr == "" {
		return nil, fmt.Errorf("addr is required")
	}

	mux := http.NewServeMux()
	setupRoutes(mux)

	httpServer := &http.Server{
		Addr:         config.Addr,
		Handler:      mux,
		ReadTimeout:  config.ReadTimeout,
		WriteTimeout: config.WriteTimeout,
		IdleTimeout:  config.IdleTimeout,
		ErrorLog:     nil, // Use structured logger instead
	}

	return &Server{
		httpServer: httpServer,
		logger:     logger,
	}, nil
}

// ListenAndServe starts the HTTP server.
func (s *Server) ListenAndServe() error {
	if s.httpServer.TLSConfig != nil {
		return s.httpServer.ListenAndServeTLS("", "")
	}
	return s.httpServer.ListenAndServe()
}

// Shutdown gracefully shuts down the server.
func (s *Server) Shutdown(ctx context.Context) error {
	return s.httpServer.Shutdown(ctx)
}

// Close immediately closes the server.
func (s *Server) Close() error {
	return s.httpServer.Close()
}

// ServeHTTP implements http.Handler.
func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	s.httpServer.Handler.ServeHTTP(w, r)
}

// SetupRoutes configures the HTTP routes.
func setupRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /health", healthHandler)
	mux.HandleFunc("GET /ready", readyHandler)
}

// healthHandler returns the health status.
func healthHandler(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	w.Write([]byte(`{"status":"healthy"}`))
}

// readyHandler returns the readiness status.
func readyHandler(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	w.Write([]byte(`{"status":"ready"}`))
}
