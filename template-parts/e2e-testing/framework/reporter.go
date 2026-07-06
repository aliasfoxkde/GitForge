// Package framework provides the Dark Factory E2E testing harness.
// This file contains reporting functionality for test results.
package framework

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// ReportFormat determines the output format.
type ReportFormat string

const (
	ReportFormatJSON  ReportFormat = "json"
	ReportFormatHTML  ReportFormat = "html"
	ReportFormatSARIF ReportFormat = "sarif"
	ReportFormatText  ReportFormat = "text"
)

// Reporter generates test reports in various formats.
type Reporter struct {
	format ReportFormat
	path   string
}

// ReporterConfig configures the reporter.
type ReporterConfig struct {
	Format ReportFormat
	Path   string
	Title  string
}

// NewReporter creates a new reporter.
func NewReporter(config ReporterConfig) *Reporter {
	return &Reporter{
		format: config.Format,
		path:   config.Path,
	}
}

// GenerateReport creates a report from test results.
func (r *Reporter) GenerateReport(results []TestResult, summary *Summary) error {
	if r.path == "" {
		r.path = defaultReportPath(r.format)
	}

	dir := filepath.Dir(r.path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("create report dir: %w", err)
	}

	switch r.format {
	case ReportFormatJSON:
		return r.writeJSON(results, summary)
	case ReportFormatHTML:
		return r.writeHTML(results, summary)
	case ReportFormatSARIF:
		return r.writeSARIF(results, summary)
	case ReportFormatText:
		return r.writeText(results, summary)
	default:
		return fmt.Errorf("unknown format: %s", r.format)
	}
}

// Summary holds aggregated test statistics.
type Summary struct {
	Total      int
	Passed     int
	Failed     int
	Skipped    int
	TotalTime  time.Duration
	Coverage   *CoverageReport
	PassRate   float64
	SessionID  string
	GeneratedAt time.Time
}

// GenerateSummary creates a summary from test results.
func GenerateSummary(results []TestResult, sessionID string) *Summary {
	s := &Summary{
		Total:      len(results),
		PassRate:   100.0,
		SessionID:  sessionID,
		GeneratedAt: time.Now(),
	}

	for _, r := range results {
		switch r.Status {
		case "pass":
			s.Passed++
		case "fail":
			s.Failed++
		case "skip":
			s.Skipped++
		}
		s.TotalTime += r.Duration
	}

	if s.Total > 0 {
		s.PassRate = float64(s.Passed) / float64(s.Total) * 100
	}

	return s
}

func defaultReportPath(format ReportFormat) string {
	ext := map[ReportFormat]string{
		ReportFormatJSON:  "json",
		ReportFormatHTML:  "html",
		ReportFormatSARIF: "sarif",
		ReportFormatText:  "txt",
	}
	return fmt.Sprintf("e2e-report.%s", ext[format])
}

func (r *Reporter) writeJSON(results []TestResult, summary *Summary) error {
	report := struct {
		Results []TestResult `json:"results"`
		Summary *Summary     `json:"summary"`
	}{
		Results: results,
		Summary: summary,
	}

	data, err := json.MarshalIndent(report, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal report: %w", err)
	}

	return os.WriteFile(r.path, data, 0644)
}

func (r *Reporter) writeHTML(results []TestResult, summary *Summary) error {
	data, err := htmlReportTemplate(results, summary)
	if err != nil {
		return fmt.Errorf("generate html: %w", err)
	}

	return os.WriteFile(r.path, data, 0644)
}

func (r *Reporter) writeSARIF(results []TestResult, summary *Summary) error {
	sarif := toSARIF(results, summary)

	data, err := json.MarshalIndent(sarif, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal sarif: %w", err)
	}

	return os.WriteFile(r.path, data, 0644)
}

func (r *Reporter) writeText(results []TestResult, summary *Summary) error {
	var b strings.Builder

	b.WriteString("E2E Test Report\n")
	b.WriteString(strings.Repeat("=", 50) + "\n\n")
	b.WriteString(fmt.Sprintf("Session: %s\n", summary.SessionID))
	b.WriteString(fmt.Sprintf("Total:   %d\n", summary.Total))
	b.WriteString(fmt.Sprintf("Passed:  %d\n", summary.Passed))
	b.WriteString(fmt.Sprintf("Failed:  %d\n", summary.Failed))
	b.WriteString(fmt.Sprintf("Skipped: %d\n", summary.Skipped))
	b.WriteString(fmt.Sprintf("Time:    %s\n\n", summary.TotalTime.Round(time.Second)))

	if summary.Failed > 0 {
		b.WriteString("Failures:\n")
		b.WriteString(strings.Repeat("-", 50) + "\n")
		for _, r := range results {
			if r.Status == "fail" {
				b.WriteString(fmt.Sprintf("  ✗ %s: %s\n", r.Name, strings.Join(r.Errors, ", ")))
			}
		}
	}

	return os.WriteFile(r.path, []byte(b.String()), 0644)
}

// SARIF report structure
type SARIF struct {
	Schema  string `json:"$schema"`
	Version string `json:"version"`
	Runs    []Run  `json:"runs"`
}

type Run struct {
	Results []Result `json:"results"`
}

type Result struct {
	RuleID    string   `json:"ruleId"`
	Level    string   `json:"level"`
	Message   string   `json:"message"`
	Locations []Location `json:"locations"`
}

type Location struct {
	URI string `json:"uri"`
}

func toSARIF(results []TestResult, summary *Summary) *SARIF {
	sarif := &SARIF{
		Schema:  "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
		Version: "2.1.0",
		Runs: []Run{
			{
				Results: make([]Result, 0),
			},
		},
	}

	for _, r := range results {
		if r.Status == "fail" {
			level := "error"
			for _, err := range r.Errors {
				if strings.Contains(strings.ToLower(err), "warning") {
					level = "warning"
					break
				}
			}

			sarif.Runs[0].Results = append(sarif.Runs[0].Results, Result{
				RuleID:  "e2e-test",
				Level:   level,
				Message:  strings.Join(r.Errors, "; "),
				Locations: []Location{
					{URI: r.Name},
				},
			})
		}
	}

	return sarif
}

func htmlReportTemplate(results []TestResult, summary *Summary) ([]byte, error) {
	var b strings.Builder
	b.WriteString(`<!DOCTYPE html>
<html>
<head>
<title>E2E Test Report</title>
<style>
body { font-family: -apple-system, sans-serif; margin: 40px; }
h1 { color: #333; }
.summary { background: #f5f5f5; padding: 20px; border-radius: 8px; margin-bottom: 20px; }
.passed { color: green; }
.failed { color: red; }
.skipped { color: #999; }
table { border-collapse: collapse; width: 100%; }
th, td { border: 1px solid #ddd; padding: 12px; text-align: left; }
th { background: #333; color: white; }
tr:nth-child(even) { background: #f9f9f9; }
</style>
</head>
<body>
<h1>E2E Test Report</h1>
<div class="summary">
<p><strong>Session:</strong> ` + summary.SessionID + `</p>
<p><strong>Total:</strong> ` + fmt.Sprintf("%d", summary.Total) + `</p>
<p class="passed"><strong>Passed:</strong> ` + fmt.Sprintf("%d", summary.Passed) + `</p>
<p class="failed"><strong>Failed:</strong> ` + fmt.Sprintf("%d", summary.Failed) + `</p>
<p class="skipped"><strong>Skipped:</strong> ` + fmt.Sprintf("%d", summary.Skipped) + `</p>
<p><strong>Duration:</strong> ` + summary.TotalTime.Round(time.Second).String() + `</p>
</div>
<table>
<tr><th>Test</th><th>Status</th><th>Duration</th></tr>
`)

	for _, r := range results {
		statusClass := strings.ToLower(r.Status)
		b.WriteString(fmt.Sprintf(`<tr><td>%s</td><td class="%s">%s</td><td>%s</td></tr>`+"\n",
			r.Name, statusClass, r.Status, r.Duration.Round(time.Millisecond)))
	}

	b.WriteString(`</table></body></html>`)
	return []byte(b.String()), nil
}
