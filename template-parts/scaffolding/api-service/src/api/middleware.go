package api

import (
	"log/slog"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/gin-gonic/gin"
)

// AuthMiddleware validates the Authorization header.
func AuthMiddleware() gin.HandlerFunc {
	return func(c *gin.Context) {
		token := c.GetHeader("Authorization")
		if token == "" {
			c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{"error": "missing authorization header"})
			return
		}

		// In production, validate JWT or API key here.
		// For scaffold, we just check it's non-empty.
		if len(token) < 8 {
			c.AbortWithStatusJSON(http.StatusUnauthorized, gin.H{"error": "invalid authorization token"})
			return
		}

		c.Next()
	}
}

// RequestLogger logs method, path, status, and latency.
func RequestLogger() gin.HandlerFunc {
	return func(c *gin.Context) {
		start := time.Now()
		path := c.Request.URL.Path
		query := c.Request.URL.RawQuery

		c.Next()

		latency := time.Since(start)
		status := c.Writer.Status()

		if query != "" {
			path = path + "?" + query
		}

		slog.Info("request",
			"method", c.Request.Method,
			"path", path,
			"status", status,
			"latency_ms", latency.Milliseconds(),
			"client_ip", c.ClientIP(),
		)
	}
}

// RateLimitMiddleware provides simple in-memory rate limiting.
func RateLimitMiddleware() gin.HandlerFunc {
	// Simple sliding window counter per IP.
	type window struct {
		count   int
		resetAt time.Time
		mu      sync.Mutex
	}
	var (
		mu             sync.Mutex
		windows        = make(map[string]*window)
		limit          = 100          // requests per window
		windowDuration = time.Minute  // window duration
	)

	cleanup := func() {
		mu.Lock()
		defer mu.Unlock()
		now := time.Now()
		for ip, w := range windows {
			w.mu.Lock()
			if now.After(w.resetAt) {
				delete(windows, ip)
			}
			w.mu.Unlock()
		}
	}

	go func() {
		ticker := time.NewTicker(windowDuration)
		for range ticker.C {
			cleanup()
		}
	}()

	return func(c *gin.Context) {
		ip := c.ClientIP()

		mu.Lock()
		w, exists := windows[ip]
		if !exists || time.Now().After(w.resetAt) {
			w = &window{resetAt: time.Now().Add(windowDuration)}
			windows[ip] = w
		}
		w.mu.Lock()
		w.count++
		count := w.count
		w.mu.Unlock()
		mu.Unlock()

		if count > limit {
			c.AbortWithStatusJSON(http.StatusTooManyRequests, gin.H{"error": "rate limit exceeded"})
			return
		}

		c.Header("X-RateLimit-Limit", strconv.Itoa(limit))
		c.Header("X-RateLimit-Remaining", strconv.Itoa(max(0, limit-count)))

		c.Next()
	}
}