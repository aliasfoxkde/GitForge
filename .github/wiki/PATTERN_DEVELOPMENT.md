# Aegis Pattern Development

[Aegis](https://github.com/aliasfoxkde) is the repository's security/quality
pattern scanner. The committed baseline (`.github/aegis-baseline.json`) pins
known findings so `make aegis` reports only *new* ones.

## Running

```bash
make aegis          # human findings vs the committed baseline
make aegis-report   # full scan, no baseline
make aegis-baseline # regenerate AFTER triaging findings
```

Scan a `git archive HEAD` tree rather than the live checkout — the baseline is
line-sensitive and the checkout may carry uncommitted drift.

```bash
aegis --format json scan .
```

(`--format json`, not `-f json`.)

## Baseline Discipline

- The baseline drifts: if `make aegis` fails on a clean checkout, regenerate it
  after triage rather than suppressing findings ad hoc
- Findings must be triaged before a baseline refresh — a silent refresh that
  absorbs new defects defeats the gate
- Changed-line scan: CI runs the aegis changed-line scan on every PR; new lines
  must not introduce unbaselined findings

## Adding Patterns

Aegis patterns are defined in its own project, not this repo. To propose one,
open an issue describing the defect class with true/false positive examples.
