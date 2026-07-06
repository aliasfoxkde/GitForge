package config

import (
	"fmt"
	"strings"

	"github.com/spf13/viper"
)

// Config holds all configuration for the service.
type Config struct {
	Host     string
	Port     int
	Env      string
	LogLevel string
	DBURL    string
}

func setDefaults(v *viper.Viper) {
	v.SetDefault("host", "0.0.0.0")
	v.SetDefault("port", 8080)
	v.SetDefault("env", "development")
	v.SetDefault("log_level", "info")
	v.SetDefault("db_url", "postgres://postgres:password@localhost:5432/apidb?sslmode=disable")
}

// Load reads configuration from file and environment variables.
// Environment variables take precedence.
func Load() (*Config, error) {
	v := viper.New()

	setDefaults(v)

	// Environment variable overrides
	v.SetEnvPrefix("APP")
	v.SetEnvKeyReplacer(strings.NewReplacer(".", "_"))
	v.AutomaticEnv()

	// Config file
	v.SetConfigName("config")
	v.SetConfigType("yaml")
	v.AddConfigPath("/etc/api-service/")
	v.AddConfigPath("$HOME/.api-service/")
	v.AddConfigPath(".")

	// Ignore error if file doesn't exist
	_ = v.ReadInConfig()

	var cfg Config
	if err := v.Unmarshal(&cfg); err != nil {
		return nil, fmt.Errorf("failed to unmarshal config: %w", err)
	}

	return &cfg, nil
}