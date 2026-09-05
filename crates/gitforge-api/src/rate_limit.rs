//! Rate limiting middleware
//!
//! Simple in-memory rate limiter for API requests.

use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use tower::Layer;

/// Rate limit configuration
#[derive(Clone, Debug)]
pub struct RateLimitConfig {
    /// Requests per minute per client
    pub requests_per_minute: u64,
    /// Burst size
    pub burst_size: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 100,
            burst_size: 20,
        }
    }
}

/// Client rate limit state
#[derive(Clone)]
struct ClientRateLimit {
    tokens: f64,
    last_update: Instant,
}

impl ClientRateLimit {
    fn new(burst_size: u64) -> Self {
        Self {
            tokens: burst_size as f64,
            last_update: Instant::now(),
        }
    }

    /// Try to consume a token, returns true if allowed
    fn try_consume(&mut self, rate: f64, burst_size: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update).as_secs_f64();
        self.last_update = now;

        // Add tokens based on elapsed time
        self.tokens = (self.tokens + elapsed * rate).min(burst_size);

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// In-memory rate limiter
#[derive(Clone)]
pub struct RateLimiter {
    clients: Arc<RwLock<HashMap<String, ClientRateLimit>>>,
    config: RateLimitConfig,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Check if a request from the given client identifier is allowed
    pub async fn check_rate_limit(&self, client_id: &str) -> bool {
        let rate = self.config.requests_per_minute as f64 / 60.0;

        let mut clients = self.clients.write().await;
        let client = clients
            .entry(client_id.to_string())
            .or_insert_with(|| ClientRateLimit::new(self.config.burst_size));

        client.try_consume(rate, self.config.burst_size as f64)
    }

    /// Get remaining requests for a client
    pub async fn remaining(&self, client_id: &str) -> u64 {
        let rate = self.config.requests_per_minute as f64 / 60.0;

        let clients = self.clients.read().await;
        if let Some(client) = clients.get(client_id) {
            let elapsed = Instant::now()
                .duration_since(client.last_update)
                .as_secs_f64();
            let tokens = (client.tokens + elapsed * rate).min(self.config.burst_size as f64);
            tokens as u64
        } else {
            self.config.burst_size
        }
    }

    /// Clean up old entries (call periodically)
    pub async fn cleanup(&self) {
        let mut clients = self.clients.write().await;
        clients.retain(|_, v| {
            let elapsed = Instant::now().duration_since(v.last_update);
            elapsed < Duration::from_secs(300) // Remove entries older than 5 minutes
        });
    }
}

/// Rate limit error response
#[derive(Debug, serde::Serialize)]
pub struct RateLimitErrorResponse {
    pub error: String,
    pub message: String,
    pub retry_after_secs: u64,
}

/// Rate limit middleware service
#[derive(Clone)]
pub struct RateLimitMiddleware<S> {
    inner: S,
    limiter: RateLimiter,
}

impl<S> RateLimitMiddleware<S> {
    pub fn new(inner: S, limiter: RateLimiter) -> Self {
        Self { inner, limiter }
    }
}

impl<S, B> tower::Service<Request<B>> for RateLimitMiddleware<S>
where
    S: tower::Service<Request<B>, Response = Response, Error = std::convert::Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let limiter = self.limiter.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Extract client identifier (IP address or user ID if authenticated)
            let client_id = extract_client_id(&request);

            // Check rate limit
            if !limiter.check_rate_limit(&client_id).await {
                // Log only a static event. No client identifier (raw IP or
                // hash) is written to application logs, preventing correlation
                // with operator-visible traffic or third-party telemetry.
                tracing::warn!("rate limit exceeded");
                let response = (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(RateLimitErrorResponse {
                        error: "rate_limit_exceeded".to_string(),
                        message: "Too many requests. Please try again later.".to_string(),
                        retry_after_secs: 60,
                    }),
                )
                    .into_response();
                return Ok(response);
            }

            inner.call(request).await
        })
    }
}

/// Layer for creating RateLimitMiddleware
#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: RateLimiter,
}

impl RateLimitLayer {
    pub fn new(limiter: RateLimiter) -> Self {
        Self { limiter }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitMiddleware::new(inner, self.limiter.clone())
    }
}

/// Extract client identifier from request
fn extract_client_id<B>(request: &Request<B>) -> String {
    // Try to get real IP from common headers
    let ip = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            request
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
        })
        .or_else(|| {
            request
                .headers()
                .get("cf-connecting-ip")
                .and_then(|v| v.to_str().ok())
        })
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // If authenticated, could use user ID instead
    // For now, use IP
    ip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_allow() {
        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 60,
            burst_size: 10,
        });

        // First request should be allowed
        assert!(limiter.check_rate_limit("test-client").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_burst() {
        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 60,
            burst_size: 5,
        });

        // Should allow burst_size requests
        for _ in 0..5 {
            assert!(limiter.check_rate_limit("test-client").await);
        }

        // Next request should be denied
        assert!(!limiter.check_rate_limit("test-client").await);
    }

    #[test]
    fn test_rate_limit_config_default() {
        let config = RateLimitConfig::default();
        assert_eq!(config.requests_per_minute, 100);
        assert_eq!(config.burst_size, 20);
    }

    #[tokio::test]
    async fn test_client_rate_limit_tokens() {
        let mut client = ClientRateLimit::new(5);

        // Should have burst_size tokens initially
        assert!(client.try_consume(1.0, 5.0)); // 5 -> 4
        assert!(client.try_consume(1.0, 5.0)); // 4 -> 3
        assert!(client.try_consume(1.0, 5.0)); // 3 -> 2
        assert!(client.try_consume(1.0, 5.0)); // 2 -> 1
        assert!(client.try_consume(1.0, 5.0)); // 1 -> 0
        assert!(!client.try_consume(1.0, 5.0)); // No tokens left
    }

    #[tokio::test]
    async fn test_extract_client_id_x_forwarded_for() {
        use axum::http::Request;

        let request = Request::builder()
            .header("x-forwarded-for", "192.168.1.100")
            .body(axum::body::Body::empty())
            .unwrap();
        let client_id = extract_client_id(&request);
        assert_eq!(client_id, "192.168.1.100");
    }

    #[tokio::test]
    async fn test_extract_client_id_x_real_ip() {
        use axum::http::Request;

        let request = Request::builder()
            .header("x-real-ip", "10.0.0.50")
            .body(axum::body::Body::empty())
            .unwrap();
        let client_id = extract_client_id(&request);
        assert_eq!(client_id, "10.0.0.50");
    }

    #[tokio::test]
    async fn test_extract_client_id_cf_connecting_ip() {
        use axum::http::Request;

        let request = Request::builder()
            .header("cf-connecting-ip", "203.0.113.50")
            .body(axum::body::Body::empty())
            .unwrap();
        let client_id = extract_client_id(&request);
        assert_eq!(client_id, "203.0.113.50");
    }

    #[tokio::test]
    async fn test_extract_client_id_multiple_forwarded_ips() {
        use axum::http::Request;

        // Should take first IP when multiple are present
        let request = Request::builder()
            .header("x-forwarded-for", "192.168.1.1, 192.168.1.2, 10.0.0.1")
            .body(axum::body::Body::empty())
            .unwrap();
        let client_id = extract_client_id(&request);
        assert_eq!(client_id, "192.168.1.1");
    }

    #[tokio::test]
    async fn test_extract_client_id_no_headers() {
        use axum::http::Request;

        let request = Request::builder().body(axum::body::Body::empty()).unwrap();
        let client_id = extract_client_id(&request);
        assert_eq!(client_id, "unknown");
    }

    #[test]
    fn test_rate_limit_layer_new() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        let layer = RateLimitLayer::new(limiter);
        let _ = layer;
    }

    #[test]
    fn test_rate_limit_config_debug() {
        let config = RateLimitConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("requests_per_minute"));
    }

    #[test]
    fn test_rate_limit_error_response_debug() {
        let error = RateLimitErrorResponse {
            error: "rate_limit_exceeded".to_string(),
            message: "Too many requests".to_string(),
            retry_after_secs: 60,
        };
        let debug_str = format!("{:?}", error);
        assert!(debug_str.contains("rate_limit_exceeded"));
    }

    #[tokio::test]
    async fn test_rate_limiter_different_clients() {
        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 60,
            burst_size: 2,
        });

        // Client 1
        assert!(limiter.check_rate_limit("client-1").await);
        assert!(limiter.check_rate_limit("client-1").await);
        assert!(!limiter.check_rate_limit("client-1").await);

        // Client 2 should still have quota
        assert!(limiter.check_rate_limit("client-2").await);
        assert!(limiter.check_rate_limit("client-2").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_remaining() {
        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 60,
            burst_size: 5,
        });

        // Initially should have burst_size remaining
        assert_eq!(limiter.remaining("test-client").await, 5);

        // After consuming, should have less
        limiter.check_rate_limit("test-client").await;
        limiter.check_rate_limit("test-client").await;
        assert_eq!(limiter.remaining("test-client").await, 3);
    }

    #[tokio::test]
    async fn test_rate_limiter_cleanup_removes_old_entries() {
        use std::time::{Duration, Instant};

        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 60,
            burst_size: 5,
        });

        // Add a client
        limiter.check_rate_limit("old-client").await;

        // Manually manipulate the client's last_update to be old
        {
            let mut clients = limiter.clients.write().await;
            if let Some(client) = clients.get_mut("old-client") {
                client.last_update = Instant::now() - Duration::from_secs(400);
            }
        }

        // Cleanup should remove the old entry
        limiter.cleanup().await;

        let clients = limiter.clients.read().await;
        assert!(clients.get("old-client").is_none());
    }

    #[tokio::test]
    async fn test_rate_limiter_preserves_recent_entries() {
        use std::time::{Duration, Instant};

        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 60,
            burst_size: 5,
        });

        // Add a client
        limiter.check_rate_limit("recent-client").await;

        // Manually manipulate the client's last_update to be recent
        {
            let mut clients = limiter.clients.write().await;
            if let Some(client) = clients.get_mut("recent-client") {
                client.last_update = Instant::now() - Duration::from_secs(60);
            }
        }

        // Cleanup should preserve the recent entry
        limiter.cleanup().await;

        let clients = limiter.clients.read().await;
        assert!(clients.get("recent-client").is_some());
    }

    #[tokio::test]
    async fn test_rate_limiter_remaining_unknown_client() {
        let limiter = RateLimiter::new(RateLimitConfig {
            requests_per_minute: 60,
            burst_size: 3,
        });

        // Unknown client should get full burst size
        let remaining = limiter.remaining("unknown-client").await;
        assert_eq!(remaining, 3);
    }
}
