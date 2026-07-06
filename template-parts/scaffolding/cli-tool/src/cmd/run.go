package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/example/cli-tool/src/config"
	"github.com/spf13/cobra"
)

var runCmd = &cobra.Command{
	Use:   "run [input]",
	Short: "Process input and produce output",
	Long: `Process the provided input and produce formatted output.

This command demonstrates a real workflow:
- Validates input data
- Processes according to configuration
- Outputs results in the specified format

Examples:
  cli-tool run "hello world"
  cli-tool run --input-file data.txt
  echo "test data" | cli-tool run`,
	Args: cobra.RangeArgs(0, 1),
	RunE: func(cmd *cobra.Command, args []string) error {
		cfg := config.GetConfig()

		// Determine input source
		var input string
		if len(args) > 0 {
			input = args[0]
		} else if inputFile != "" {
			data, err := os.ReadFile(inputFile)
			if err != nil {
				return fmt.Errorf("failed to read input file: %w", err)
			}
			input = string(data)
		} else {
			// Read from stdin
			data, err := os.ReadAll(os.Stdin)
			if err != nil {
				return fmt.Errorf("failed to read stdin: %w", err)
			}
			input = strings.TrimSpace(string(data))
		}

		if input == "" {
			return fmt.Errorf("no input provided")
		}

		// Process the input
		start := time.Now()
		result, err := processInput(input, cfg)
		if err != nil {
			return fmt.Errorf("processing failed: %w", err)
		}
		duration := time.Since(start)

		// Output results
		return outputResult(result, duration, cfg)
	},
}

var (
	inputFile string
	timeout   int
)

func init() {
	rootCmd.AddCommand(runCmd)
	runCmd.Flags().StringVar(&inputFile, "input-file", "", "input file path")
	runCmd.Flags().IntVar(&timeout, "timeout", 30, "timeout in seconds for processing")
}

func processInput(input string, cfg config.Config) (string, error) {
	if cfg.Verbose {
		fmt.Fprintf(os.Stderr, "Processing input of length %d...\n", len(input))
	}

	// Simulate processing work
	// In a real application, this would contain the actual business logic
	processed := strings.ToUpper(input)

	// Simulate some processing time (remove in real usage)
	time.Sleep(100 * time.Millisecond)

	if cfg.Verbose {
		fmt.Fprintf(os.Stderr, "Processing complete.\n")
	}

	return processed, nil
}

func outputResult(result string, duration time.Duration, cfg config.Config) error {
	output := struct {
		Result   string `json:"result"`
		Duration string `json:"duration"`
		Format   string `json:"format"`
	}{
		Result:   result,
		Duration: duration.String(),
		Format:   cfg.OutputFormat,
	}

	switch cfg.OutputFormat {
	case "json":
		encoder := json.NewEncoder(os.Stdout)
		encoder.SetIndent("", "  ")
		return encoder.Encode(output)
	case "yaml":
		fmt.Printf("result: %s\n", result)
		fmt.Printf("duration: %s\n", duration.String())
		fmt.Printf("format: %s\n", cfg.OutputFormat)
		return nil
	default:
		fmt.Printf("Result: %s\n", result)
		fmt.Printf("Duration: %s\n", duration.String())
		return nil
	}
}
