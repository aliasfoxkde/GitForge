// Package otel provides OpenTelemetry tracing setup and propagation for Go.
// Purpose: Distributed tracing for observability across service boundaries.
// Usage:
//   tp, shutdown := otel.SetupTracing(ctx, "myservice", "localhost:4317")
   defer shutdown()
//   span := otel.StartSpan(ctx, "operation-name")
//   defer span.End()
//   // ... do work
//   span.RecordError(err)
//
// Dependencies: go.opentelemetry.io/otel, go.opentelemetry.io/otel/trace
// Install: go get go.opentelemetry.io/otel@latest
package otel

import (
	"context"
	"errors"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracegrpc"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/sdk/resource"
	sdktrace "go.opentelemetry.io/otel/sdk/trace"
	semconv "go.opentelemetry.io/otel/semconv/v1.26.0"
	"go.opentelemetry.io/otel/trace"
)

// Config holds tracing configuration.
type Config struct {
	ServiceName    string
	ServiceVersion string
	Endpoint       string // OTLP gRPC endpoint (e.g., "localhost:4317")
	Enabled        bool   // Set to false to disable tracing
}

// DefaultConfig returns sensible defaults.
func DefaultConfig(serviceName string) Config {
	return Config{
		ServiceName:    serviceName,
		ServiceVersion: "unknown",
		Endpoint:       "localhost:4317",
		Enabled:        true,
	}
}

// TracerProvider holds the trace provider and cleanup function.
type TracerProvider struct {
	TP      *sdktrace.TracerProvider
	Shutdown func(context.Context) error
}

// SetupTracing initializes the OpenTelemetry tracer provider.
// Returns the provider and shutdown function.
func SetupTracing(ctx context.Context, cfg Config) (*TracerProvider, error) {
	if !cfg.Enabled {
		return &TracerProvider{}, nil
	}

	if cfg.ServiceName == "" {
		return nil, errors.New("service name is required")
	}

	// Create OTLP exporter
	exporter, err := otlptracegrpc.New(ctx,
		otlptracegrpc.WithEndpoint(cfg.Endpoint),
		otlptracegrpc.WithInsecure(), // Use TLS in production
	)
	if err != nil {
		return nil, err
	}

	// Create resource with service info
	res, err := resource.New(ctx,
		resource.WithAttributes(
			semconv.ServiceName(cfg.ServiceName),
			semconv.ServiceVersion(cfg.ServiceVersion),
		),
	)
	if err != nil {
		return nil, err
	}

	// Create trace provider
	tp := sdktrace.NewTracerProvider(
		sdktrace.WithBatcher(exporter),
		sdktrace.WithResource(res),
		sdktrace.WithSampler(sdktrace.AlwaysSample()),
	)

	// Set global tracer provider and propagator
	otel.SetTracerProvider(tp)
	otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(
		propagation.TraceContext{},
		propagation.Baggage{},
	))

	return &TracerProvider{
		TP: tp,
		Shutdown: func(ctx context.Context) error {
			return tp.Shutdown(ctx)
		},
	}, nil
}

// StartSpan starts a new span with the given name.
// Automatically sets the span as active in the context.
func StartSpan(ctx context.Context, name string, opts ...trace.SpanStartOption) (context.Context, trace.Span) {
	tracer := otel.Tracer("")
	return tracer.Start(ctx, name, opts...)
}

// StartSpanWithAttrs starts a span with additional attributes.
func StartSpanWithAttrs(ctx context.Context, name string, attrs []attribute.KeyValue) (context.Context, trace.Span) {
	opts := []trace.SpanStartOption{
		trace.WithAttributes(attrs...),
	}
	return StartSpan(ctx, name, opts...)
}

// RecordError records an error on the span.
func RecordError(span trace.Span, err error, attrs ...attribute.KeyValue) {
	if err == nil {
		return
	}

	recordAttrs := append([]attribute.KeyValue{
		attribute.String("error.type", "error"),
		attribute.String("error.message", err.Error()),
	}, attrs...)

	span.SetAttributes(recordAttrs...)
	span.RecordError(err)
}

// AddEvent adds a named event to the span.
func AddEvent(span trace.Span, name string, attrs ...attribute.KeyValue) {
	span.AddEvent(name, trace.WithAttributes(attrs...))
}

// SetAttribute sets a single attribute on the span.
func SetAttribute(span trace.Span, key string, value attribute.Value) {
	span.SetAttributes(attribute.KeyValue{Key: attribute.Key(key), Value: value})
}

// InjectTraceContext injects trace context into a carrier map for propagation.
func InjectTraceContext(ctx context.Context, carrier map[string]string) {
	otel.GetTextMapPropagator().Inject(ctx, propagation.MapCarrier(carrier))
}

// ExtractTraceContext extracts trace context from a carrier map.
func ExtractTraceContext(ctx context.Context, carrier map[string]string) context.Context {
	return otel.GetTextMapPropagator().Extract(ctx, propagation.MapCarrier(carrier))
}

// SpanFromContext returns the current span from the context.
func SpanFromContext(ctx context.Context) trace.Span {
	return trace.SpanFromContext(ctx)
}

// IsRecording returns true if the span is being recorded.
func IsRecording(ctx context.Context) bool {
	span := trace.SpanFromContext(ctx)
	return span.IsRecording()
}

// Attributes helper for common attribute types.
var (
	AttrString  = attribute.String
	AttrInt     = attribute.Int64
	AttrFloat   = attribute.Float64
	AttrBool    = attribute.Bool
	AttrIntSlice = func(k string, v []int) attribute.KeyValue {
		return attribute.Int64Slice(k, toInt64Slice(v))
	}
)

func toInt64Slice(v []int) []int64 {
	result := make([]int64, len(v))
	for i, n := range v {
		result[i] = int64(n)
	}
	return result
}
