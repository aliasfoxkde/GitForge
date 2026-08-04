# Repository Ruleset Template

This template part configures repository guardrails (rulesets) for code quality and safety enforcement.

## Features

- **Safeguards Ruleset**: Imported from Atheon-Enhanced patterns
  - Branch deletion prevention
  - Non-fast-forward merge prevention  
  - Required linear history
  - PR requirements (thread resolution, merge methods)
  - CodeQL integration
  - Code quality checks

## Files

```
repository-ruleset/
├── script/
│   └── setup-ruleset.sh    # Main setup script
└── README.md               # This file
```

## Usage

### Automatic (via setup-repo.sh)

When using `dark-factory/scripts/setup-repo.sh`, the ruleset is automatically configured.

### Manual Setup

```bash
# Run the setup script
./template-parts/scaffolding/repository-ruleset/script/setup-ruleset.sh owner/repo
```

## Customization

Edit `script/setup-ruleset.sh` to modify:
- Ruleset name
- Branch patterns to include/exclude
- Required rules
- Enforcement level

## Requirements

- `gh` CLI authenticated
- Repository admin access

## See Also

- [Atheon-Enhanced Ruleset](https://github.com/aliasfoxkde/Atheon-Enhanced/settings/rules/17952127)
- [GitHub Ruleset API](https://docs.github.com/rest/repos/rules)