// Package config handles configuration management with viper
package config

import (
	"fmt"
	"os"
	"strings"

	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

// Config holds all application configuration
type Config struct {
	ConfigPath    string
	Verbose       bool
	OutputFormat  string
	Timeout       int
	Environment   string
}

// Viper instance for configuration management
var Viper = viper.New()

// Global config instance
var globalConfig Config

// InitConfig initializes viper and binds environment variables and flags
func InitConfig() {
	// Set defaults
	Viper.SetDefault("verbose", false)
	Viper.SetDefault("output_format", "text")
	Viper.SetDefault("timeout", 30)
	Viper.SetDefault("environment", "development")

	// Allow environment variables to override config
	// Environment variables use underscore prefix and are uppercase
	Viper.SetEnvPrefix("APP")
	Viper.SetEnvKeyReplacer(strings.NewReplacer(".", "_"))
	Viper.AutomaticEnv()

	// Bind environment variables explicitly for common config
	bindEnvVars()

	// Read config file if specified
	configPath := os.Getenv("CONFIG_PATH")
	if configPath != "" {
		Viper.SetConfigFile(configPath)
	} else {
		// Try common config file locations
		Viper.SetConfigName("config")
		Viper.SetConfigType("yaml")
		Viper.AddConfigPath(".")
		Viper.AddConfigPath("./config")
		Viper.AddConfigPath("$HOME/.config/cli-tool")
		Viper.AddConfigPath("/etc/cli-tool")
	}

	// Ignore the config file not found error - defaults will be used
	_ = Viper.ReadInConfig()

	// Load config into global struct
	loadConfig()
}

// bindEnvVars explicitly binds important environment variables
func bindEnvVars() {
	// CONFIG_PATH - path to config file
	_ = Viper.BindEnv("config_path", "CONFIG_PATH")

	// VERBOSE - enable verbose output
	_ = Viper.BindEnv("verbose", "VERBOSE")

	// OUTPUT_FORMAT - output format (text, json, yaml)
	_ = Viper.BindEnv("output_format", "OUTPUT_FORMAT")

	// TIMEOUT - timeout in seconds
	_ = Viper.BindEnv("timeout", "TIMEOUT")

	// ENVIRONMENT - environment (development, staging, production)
	_ = Viper.BindEnv("environment", "ENVIRONMENT")
}

// loadConfig loads configuration from viper into the global Config struct
func loadConfig() {
	globalConfig = Config{
		ConfigPath:   Viper.GetString("config"),
		Verbose:      Viper.GetBool("verbose"),
		OutputFormat: Viper.GetString("output_format"),
		Timeout:      Viper.GetInt("timeout"),
		Environment:  Viper.GetString("environment"),
	}
}

// GetConfig returns the current configuration
func GetConfig() Config {
	return globalConfig
}

// GetConfigPath returns the config file path
func GetConfigPath() string {
	return globalConfig.ConfigPath
}

// IsVerbose returns whether verbose mode is enabled
func IsVerbose() bool {
	return globalConfig.Verbose
}

// GetOutputFormat returns the current output format
func GetOutputFormat() string {
	return globalConfig.OutputFormat
}

// GetTimeout returns the timeout in seconds
func GetTimeout() int {
	return globalConfig.Timeout
}

// ValidateConfig validates the configuration and returns any errors
func ValidateConfig() error {
	// Validate output format
	validFormats := map[string]bool{"text": true, "json": true, "yaml": true}
	if !validFormats[globalConfig.OutputFormat] {
		return fmt.Errorf("invalid output format: %s (valid: text, json, yaml)", globalConfig.OutputFormat)
	}

	// Validate timeout
	if globalConfig.Timeout < 0 {
		return fmt.Errorf("timeout must be non-negative, got: %d", globalConfig.Timeout)
	}

	// Validate environment
	validEnvironments := map[string]bool{"development": true, "staging": true, "production": true}
	if !validEnvironments[globalConfig.Environment] {
		return fmt.Errorf("invalid environment: %s (valid: development, staging, production)", globalConfig.Environment)
	}

	return nil
}

// BindFlags binds command-line flags to viper
// This is called from command init() functions
func BindFlags(cmd *cobra.Command) {
	// This is called by commands to bind their specific flags
	// The root flags are bound in root.go
	_ = cmd.ParseFlags(os.Args)
}
