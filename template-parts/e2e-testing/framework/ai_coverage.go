package framework

import (
	"fmt"
	"os"
)

// AICoverageAnalyzer uses AI to identify coverage gaps and suggest tests.
type AICoverageAnalyzer struct {
	model   string
	mode    bool
}

// NewAICoverageAnalyzer creates a new AI coverage analyzer.
func NewAICoverageAnalyzer(model string, mode bool) *AICoverageAnalyzer {
	return &AICoverageAnalyzer{
		model: model,
		mode:  mode,
	}
}

// Analyze reviews coverage data and identifies untested code paths.
// Returns an error if coverage is below targets.
func (a *AICoverageAnalyzer) Analyze(cov *CoverageReport, target CoverageTarget) error {
	if cov == nil {
		return fmt.Errorf("no coverage data available")
	}

	// Check against targets
	var failures []string

	if cov.Branch < target.Branch {
		failures = append(failures,
			fmt.Sprintf("branch coverage %.1f%% < target %.1f%%",
				cov.Branch*100, target.Branch*100))
	}

	if cov.Function < target.Function {
		failures = append(failures,
			fmt.Sprintf("function coverage %.1f%% < target %.1f%%",
				cov.Function*100, target.Function*100))
	}

	if cov.Line < target.Line {
		failures = append(failures,
			fmt.Sprintf("line coverage %.1f%% < target %.1f%%",
				cov.Line*100, target.Line*100))
	}

	if len(failures) > 0 {
		// Try to get AI suggestions for uncovered code
		if a.mode && len(cov.Uncovered) > 0 {
			suggestions := a.suggestTestsForUncovered(cov.Uncovered)
			fmt.Fprintf(os.Stderr,
				"[ai-coverage] Coverage below target:\n  %s\n\nAI suggestions:\n%s\n",
				suggestions)
		}

		return fmt.Errorf("coverage targets not met: %v", failures)
	}

	return nil
}

func (a *AICoverageAnalyzer) suggestTestsForUncovered(blocks []UncoveredBlock) string {
	// TODO: Integrate with AI model (Claude API, etc.) to generate test suggestions
	// For now, return a placeholder that can be enhanced with actual AI calls

	var suggestion string
	for _, block := range blocks {
		if len(suggestion) > 2000 {
			break
		}
		suggestion += fmt.Sprintf("  - %s:%d (%s) — consider adding test for %s\n",
			block.File, block.Line, block.Function, block.Reason)
	}
	return suggestion
}
