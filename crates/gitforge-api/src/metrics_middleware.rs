//! HTTP metrics middleware
//!
//! Intercepts all HTTP requests and records metrics for Prometheus export.

use crate::metrics::Metrics;
use axum::{extract::Request, response::Response};
use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tower::{Layer, Service};

/// Middleware service that records HTTP metrics
#[derive(Clone)]
pub struct MetricsMiddleware<S> {
    inner: S,
    metrics: Arc<Metrics>,
}

impl<S> MetricsMiddleware<S> {
    /// Create a new metrics middleware
    pub fn new(inner: S, metrics: Arc<Metrics>) -> Self {
        Self { inner, metrics }
    }
}

impl<S> Service<Request> for MetricsMiddleware<S>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Metrics middleware is always ready
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let metrics = self.metrics.clone();
        let mut inner = self.inner.clone();

        // Normalize the path for metrics (replace UUIDs with {id})
        let path = normalize_path(request.uri().path());

        // Record start time
        let start = std::time::Instant::now();
        let method = request.method().as_str().to_string();

        // We need to use a separate future that captures method and path
        Box::pin(async move {
            let response = inner.call(request).await;

            // Record metrics
            let duration = start.elapsed().as_secs_f64();
            // Handle case where inner service returns error (shouldn't happen with Infallible)
            let status = response.as_ref().map(|r| r.status()).unwrap_or_default();

            metrics.record_http_request(&method, &path, status.as_u16());
            metrics.record_http_duration(&method, &path, duration);

            response
        })
    }
}

/// Layer for creating MetricsMiddleware
#[derive(Clone)]
pub struct MetricsLayer {
    metrics: Arc<Metrics>,
}

impl MetricsLayer {
    /// Create a new metrics layer
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self { metrics }
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetricsMiddleware::new(inner, self.metrics.clone())
    }
}

/// Normalize a path by replacing UUIDs and numeric IDs with placeholders.
///
/// This prevents high-cardinality labels in metrics.
/// Example: `/api/repos/550e8400-e29b-41d4-a716-446655440000` -> `/api/repos/{id}`
/// Example: `/api/repos/123` -> `/api/repos/{id}`
fn normalize_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let normalized: Vec<String> = segments
        .iter()
        .skip(1) // Skip empty string before leading '/'
        .map(|segment| {
            if is_id_segment(segment) {
                "{id}".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect();
    format!("/{}", normalized.join("/"))
}

/// Check if a path segment looks like an ID (UUID or numeric)
fn is_id_segment(segment: &str) -> bool {
    // Check for UUID pattern
    if is_uuid(segment) {
        return true;
    }

    // Check for pure numeric ID
    if segment.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    false
}

/// Check if a string is a UUID
fn is_uuid(s: &str) -> bool {
    // UUID format: 8-4-4-4-12 hex digits
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }

    let lengths = [8, 4, 4, 4, 12];
    for (part, &len) in parts.iter().zip(lengths.iter()) {
        if part.len() != len || !part.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_with_uuid() {
        assert_eq!(
            normalize_path("/api/repos/550e8400-e29b-41d4-a716-446655440000"),
            "/api/repos/{id}"
        );
    }

    #[test]
    fn test_normalize_path_with_numeric_id() {
        assert_eq!(normalize_path("/api/repos/12345"), "/api/repos/{id}");
    }

    #[test]
    fn test_normalize_path_preserves_static_segments() {
        assert_eq!(normalize_path("/api/repos"), "/api/repos");
    }

    #[test]
    fn test_normalize_path_nested_uuid() {
        assert_eq!(
            normalize_path("/api/repos/550e8400-e29b-41d4-a716-446655440000/artifacts"),
            "/api/repos/{id}/artifacts"
        );
    }

    #[test]
    fn test_is_uuid_valid() {
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_uuid("00000000-0000-0000-0000-000000000000"));
    }

    #[test]
    fn test_is_uuid_invalid() {
        assert!(!is_uuid("not-a-uuid"));
        assert!(!is_uuid("550e8400-e29b-41d4-a716")); // too short
        assert!(!is_uuid("550e8400-e29b-41d4-a716-4466554400001")); // too long
        assert!(!is_uuid("550e8400-e29b-41d4g-a716-446655440000")); // non-hex char
    }

    #[test]
    fn test_is_id_segment() {
        assert!(is_id_segment("123"));
        assert!(is_id_segment("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_id_segment("repos"));
        assert!(!is_id_segment("health"));
    }
}
