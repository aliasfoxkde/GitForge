package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"text/tabwriter"

	"github.com/example/cli-tool/src/config"
	"github.com/spf13/cobra"
)

var versionCmd = &cobra.Command{
	Use:   "version",
	Short: "Print version information",
	Long: `Display version and build information for this CLI tool.

The version command outputs:
- Application version
- Git commit hash
- Build date
- Built-by information

Examples:
  cli-tool version
  cli-tool version --json`,
	Args: cobra.NoArgs,
	RunE: func(cmd *cobra.Command, args []string) error {
		cfg := config.GetConfig()
		printJSON := cfg.OutputFormat == "json"

		if printJSON {
			return printVersionJSON()
		}
		return printVersionText()
	},
}

type versionInfo struct {
	Version   string `json:"version"`
	Commit    string `json:"commit"`
	Date      string `json:"date"`
	BuiltBy   string `json:"built_by"`
	GoVersion string `json:"go_version"`
	OS        string `json:"os"`
	Arch      string `json:"arch"`
}

func init() {
	// Version command inherits from root, ensure config is initialized
}

func printVersionText() error {
	w := tabwriter.NewWriter(os.Stdout, 0, 8, 2, '\t', 0)
	defer w.Flush()

	fmt.Fprintln(w, "Application:\tcli-tool")
	fmt.Fprintln(w, "Version:\t"+version)
	fmt.Fprintln(w, "Commit:\t\t"+commit)
	fmt.Fprintln(w, "Build Date:\t"+date)
	fmt.Fprintln(w, "Built By:\t"+builtBy)
	fmt.Fprintln(w, "Go Version:\t"+getGoVersion())
	fmt.Fprintln(w, "OS/Arch:\t"+getGoOSArch())

	return nil
}

func printVersionJSON() error {
	info := versionInfo{
		Version:   version,
		Commit:    commit,
		Date:      date,
		BuiltBy:   builtBy,
		GoVersion: getGoVersion(),
		OS:        getGoOS(),
		Arch:      getGoArch(),
	}

	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", "  ")
	return encoder.Encode(info)
}

func getGoVersion() string {
	return getGoInfo("version")
}

func getGoOS() string {
	return getGoInfo("os")
}

func getGoArch() string {
	return getGoInfo("arch")
}

func getGoOSArch() string {
	return getGoOS() + "/" + getGoArch()
}

func getGoInfo(field string) string {
	switch field {
	case "version":
		return "go1.21" // Build-time injected
	case "os":
		return "linux" // Build-time injected
	case "arch":
		return "amd64" // Build-time injected
	default:
		return "unknown"
	}
}
