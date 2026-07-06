// CLI Tool application entry point
package main

import (
	"os"

	"github.com/example/cli-tool/cmd"
)

func main() {
	if err := cmd.Execute(); err != nil {
		os.Exit(1)
	}
}
