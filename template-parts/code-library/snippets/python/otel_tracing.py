"""OpenTelemetry tracing setup and utilities for Python.

Distributed tracing for observability across service boundaries.

Usage:
    # Setup
    tracer_provider = setup_tracing("myservice", "localhost:4317")
    try:
        with tracer.start_as_current_span("operation") as span:
            span.set_attribute("user.id", "123")
            do_work()
    finally:
        tracer_provider.shutdown()

Dependencies:
    opentelemetry-api, opentelemetry-sdk, opentelemetry-exporter-otlp
    pip install opentelemetry-api opentelemetry-sdk opentelemetry-exporter-otlp
"""

from __future__ import annotations

import logging
from typing import Any, Iterator

from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource, SERVICE_NAME, SERVICE_VERSION
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, ConsoleSpanExporter
from opentelemetry.trace import Span, Status, StatusCode
from opentelemetry.trace.propagation import set_span_in_context
from opentelemetry.context import Context

logger = logging.getLogger(__name__)


class TracingConfig:
    """Configuration for tracing setup."""

    def __init__(
        self,
        service_name: str,
        service_version: str = "unknown",
        endpoint: str = "localhost:4317",
        enabled: bool = True,
        console_export: bool = False,
    ) -> None:
        self.service_name = service_name
        self.service_version = service_version
        self.endpoint = endpoint
        self.enabled = enabled
        self.console_export = console_export


def setup_tracing(config: TracingConfig) -> trace.TracerProvider:
    """Initialize OpenTelemetry tracing.

    Args:
        config: Tracing configuration.

    Returns:
        TracerProvider that should be shutdown when done.
    """
    if not config.enabled:
        # Return a no-op provider
        return trace.get_tracer_provider()

    # Create resource with service info
    resource = Resource.create({
        SERVICE_NAME: config.service_name,
        SERVICE_VERSION: config.service_version,
    })

    # Create tracer provider
    provider = TracerProvider(resource=resource)

    # Add OTLP exporter for production
    try:
        otlp_exporter = OTLPSpanExporter(
            endpoint=config.endpoint,
            insecure=True,  # Use TLS in production
        )
        provider.add_span_processor(BatchSpanProcessor(otlp_exporter))
    except Exception as e:
        logger.warning(f"Failed to setup OTLP exporter: {e}")

    # Optional console exporter for debugging
    if config.console_export:
        console_exporter = ConsoleSpanExporter()
        provider.add_span_processor(BatchSpanProcessor(console_exporter))

    # Set global tracer provider
    trace.set_tracer_provider(provider)

    return provider


def get_tracer(name: str = __name__) -> trace.Tracer:
    """Get a tracer instance."""
    return trace.get_tracer(name)


def get_current_span() -> Span:
    """Get the current active span."""
    return trace.get_current_span()


def start_span(name: str, **kwargs: Any) -> contextlib.contextmanager:
    """Start a new span as current context.

    Usage:
        with start_span("my-operation", attributes={"key": "value"}):
            do_work()
    """
    tracer = get_tracer()
    return tracer.start_as_current_span(name, **kwargs)


def record_exception(span: Span, exception: Exception, attributes: dict[str, Any] | None = None) -> None:
    """Record an exception on a span.

    Args:
        span: The span to record on.
        exception: The exception to record.
        attributes: Additional attributes to set.
    """
    span.record_exception(exception, attributes=attributes)
    span.set_status(Status(StatusCode.ERROR, str(exception)))

    if attributes:
        for key, value in attributes.items():
            span.set_attribute(key, value)


def set_span_attribute(span: Span, key: str, value: Any) -> None:
    """Set an attribute on a span."""
    span.set_attribute(key, value)


def add_span_event(span: Span, name: str, attributes: dict[str, Any] | None = None) -> None:
    """Add an event to a span."""
    span.add_event(name, attributes=attributes or {})


def inject_trace_context(carrier: dict[str, str]) -> None:
    """Inject current trace context into a carrier (e.g., HTTP headers).

    Args:
        carrier: Dict to inject trace context into (e.g., request headers).
    """
    propagator = trace.get_global_textmap_propaginator()
    ctx = trace.get_current_span_data().context if trace.get_current_span().get_span_context().is_valid else Context()
    propagator.inject(carrier, context=ctx)


def extract_trace_context(carrier: dict[str, str]):  # -> Context:
    """Extract trace context from a carrier.

    Args:
        carrier: Dict containing trace context (e.g., HTTP headers).
    """
    propagator = trace.get_global_textmap_propaginator()
    return propagator.extract(carrier=carrier)


def create_child_context(parent_span: Span) -> Context:
    """Create a child context from a parent span."""
    return set_span_in_context(parent_span)


# Context manager for span creation
import contextlib


@contextlib.contextmanager
def span(
    name: str,
    attributes: dict[str, Any] | None = None,
    kind: trace.SpanKind = trace.SpanKind.INTERNAL,
) -> Iterator[Span]:
    """Context manager for creating a span.

    Usage:
        with span("my-operation", attributes={"user.id": "123"}) as span:
            span.set_attribute("processing", True)
            do_work()
    """
    tracer = get_tracer()

    with tracer.start_as_current_span(
        name,
        kind=kind,
        attributes=attributes,
    ) as span:
        try:
            yield span
        except Exception as e:
            record_exception(span, e)
            raise
