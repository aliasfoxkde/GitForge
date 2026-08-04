// Package fixtures provides test helpers and example test patterns.
package fixtures_test

import (
	"context"
	"errors"
	"testing"
	"time"
)

// Example table-driven test pattern
func TestAdd(t *testing.T) {
	tests := []struct {
		name string
		a, b int
		want int
	}{
		{"positive numbers", 1, 2, 3},
		{"negative numbers", -1, -2, -3},
		{"zero", 0, 0, 0},
		{"mixed signs", -1, 2, 1},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := Add(tt.a, tt.b)
			if got != tt.want {
				t.Errorf("Add(%d, %d) = %d, want %d", tt.a, tt.b, got, tt.want)
			}
		})
	}
}

// Example subtest pattern with shared setup
func TestService(t *testing.T) {
	svc := NewService()

	tests := []struct {
		name    string
		input   string
		setup   func(*Service)
		wantErr bool
	}{
		{
			name:    "valid input",
			input:   "test",
			setup:   func(s *Service) {},
			wantErr: false,
		},
		{
			name:    "empty input",
			input:   "",
			setup:   func(s *Service) {},
			wantErr: true,
		},
		{
			name:    "with mock",
			input:   "mock",
			setup:   func(s *Service) { s.mockEnabled = true },
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tt.setup(svc)
			err := svc.Process(tt.input)
			if (err != nil) != tt.wantErr {
				t.Errorf("Process() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

// Example context propagation test
func TestWithContext(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	svc := NewService()

	err := svc.ProcessWithContext(ctx, "test")
	if err != nil {
		t.Errorf("ProcessWithContext() error = %v", err)
	}

	// Test context cancellation
	ctx2, cancel2 := context.WithCancel(context.Background())
	cancel2()

	err = svc.ProcessWithContext(ctx2, "canceled")
	if !errors.Is(err, context.Canceled) {
		t.Errorf("expected context.Canceled, got %v", err)
	}
}

// Example error handling pattern
func TestErrors(t *testing.T) {
	tests := []struct {
		name string
		err  error
		want string
	}{
		{"ErrNotFound", ErrNotFound, "not found"},
		{"ErrInvalidInput", ErrInvalidInput, "invalid input"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.err.Error() != tt.want {
				t.Errorf("err.Error() = %q, want %q", tt.err.Error(), tt.want)
			}
		})
	}
}

// Example benchmark pattern
func BenchmarkAdd(b *testing.B) {
	for i := 0; i < b.N; i++ {
		Add(1, 2)
	}
}

// Example: Helper functions for tests (implement these in your code)

// Add returns the sum of two integers.
func Add(a, b int) int {
	return a + b
}

// Service is an example service struct.
type Service struct {
	mockEnabled bool
}

// NewService creates a new service.
func NewService() *Service {
	return &Service{}
}

// Process processes an input string.
func (s *Service) Process(input string) error {
	if input == "" {
		return ErrInvalidInput
	}
	return nil
}

// ProcessWithContext processes with context support.
func (s *Service) ProcessWithContext(ctx context.Context, input string) error {
	select {
	case <-ctx.Done():
		return ctx.Err()
	default:
		return s.Process(input)
	}
}

// Sentinel errors
var (
	ErrNotFound     = errors.New("not found")
	ErrInvalidInput = errors.New("invalid input")
)
