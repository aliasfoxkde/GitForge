// Package config provides application configuration management.
package config

import (
	"os"
	"strconv"
	"time"
)

// Config holds all application configuration.
type Config struct {
	Version string
	Env     string

	HTTPPort         int
	HTTPPort         int
	ReadTimeout      time.Duration
	WriteTimeout     time.Duration
	IdleTimeout      time.Duration
	ShutdownTimeout  time.Duration

	DatabaseURL string
	DatabaseMaxOpen int
	DatabaseMaxIdle int
	DatabaseMaxLife time.Duration

	LogLevel  string
	LogFormat string
}

// LoadConfig reads configuration from environment variables.
// All configuration MUST be via environment variables (12-factor app).
func LoadConfig() *Config {
	return &Config{
		Version:        getEnv("APP_VERSION", "dev"),
		Env:            getEnv("APP_ENV", "development"),
		HTTPPort:       getEnvInt("HTTP_PORT", 8080),
		ReadTimeout:    getEnvDuration("HTTP_READ_TIMEOUT", 30*time.Second),
		WriteTimeout:   getEnvDuration("HTTP_WRITE_TIMEOUT", 30*time.Second),
		IdleTimeout:    getEnvDuration("HTTP_IDLE_TIMEOUT", 120*time.Second),
		ShutdownTimeout:getEnvDuration("SHUTDOWN_TIMEOUT", 30*time.Second),
		DatabaseURL:    getEnv("DATABASE_URL", ""),
		DatabaseMaxOpen: getEnvInt("DB_MAX_OPEN", 25),
		DatabaseMaxIdle: getEnvInt("DB_MAX_IDLE", 5),
		DatabaseMaxLife: getEnvDuration("DB_MAX_LIFE", 5*time.Minute),
		LogLevel:        getEnv("LOG_LEVEL", "info"),
		LogFormat:       getEnv("LOG_FORMAT", "json"),
	}
}

func getEnv(key, defaultValue string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return defaultValue
}

func getEnvInt(key string, defaultValue int) int {
	if v := os.Getenv(key); v != "" {
		if i, err := strconv.Atoi(v); err == nil {
			return i
		}
	}
	return defaultValue
}

func getEnvDuration(key string, defaultValue time.Duration) time.Duration {
	if v := os.Getenv(key); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			return d
		}
	}
	return defaultValue
}
