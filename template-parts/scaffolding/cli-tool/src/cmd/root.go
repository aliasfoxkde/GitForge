// Package cmd implements the CLI commands using Cobra.
package cmd

import (
	"fmt"
	"os"

	"github.com/example/cli-tool/src/config"
	"github.com/spf13/cobra"
)

// Version info set at build time via ldflags
var (
	version   = "dev"
	commit    = "unknown"
	date      = "unknown"
	builtBy   = "unknown"
)

// rootCmd represents the base command when called without any subcommands
var rootCmd = &cobra.Command{
	Use:   "cli-tool",
	Short: "A brief description of your CLI tool",
	Long: `A longer description that spans multiple lines and likely contains
examples and usage of using your application.

This CLI tool demonstrates best practices for building production-ready
command-line applications with Cobra, including configuration management,
persistent flags, and proper error handling.`,
	SilenceUsage: true,
}

// Execute adds all child commands to the root command and sets flags appropriately.
// This is called by main.main(). It only needs to happen once to the rootCmd.
func Execute() error {
	return rootCmd.Execute()
}

func init() {
	// Bind viper to config flags and environment variables
	config.InitConfig()

	// Persistent flags for the root command
	// These flags are available to all subcommands
	rootCmd.PersistentFlags().StringP("config", "c", "", "config file path (default is ./config.yaml or $CONFIG_PATH)")
	rootCmd.PersistentFlags().BoolP("verbose", "v", false, "enable verbose output")
	rootCmd.PersistentFlags().StringP("output-format", "o", "text", "output format (text, json, yaml)")

	// Bind viper to these flags
	if err := config.Viper.BindPFlag("config", rootCmd.PersistentFlags().Lookup("config")); err != nil {
		fmt.Fprintf(os.Stderr, "Error binding config flag: %v\n", err)
	}
	if err := config.Viper.BindPFlag("verbose", rootCmd.PersistentFlags().Lookup("verbose")); err != nil {
		fmt.Fprintf(os.Stderr, "Error binding verbose flag: %v\n", err)
	}
	if err := config.Viper.BindPFlag("output_format", rootCmd.PersistentFlags().Lookup("output-format")); err != nil {
		fmt.Fprintf(os.Stderr, "Error binding output-format flag: %v\n", err)
	}

	// Register version command
	rootCmd.AddCommand(versionCmd)
}
