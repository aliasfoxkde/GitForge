# Pattern Development

This project uses a pattern-based approach for detection and quality enforcement.

## Pattern Format

Patterns are defined in YAML:

```yaml
name: pattern-name
description: What this pattern detects
category: security|quality|best-practice
match:
  type: regex|exact|contains
  pattern: "the pattern to match"
enabled: true
```

## Adding a New Pattern

1. Create a YAML file in `community/patterns/`
2. Add `name`, `match`, and `category`
3. Run `go run ./bundler` to rebuild the bundle
4. Add tests for true positives and false positives
5. Update documentation

## Pattern Categories

- `security` — Security vulnerabilities
- `quality` — Code quality issues
- `best-practice` — Recommended patterns
