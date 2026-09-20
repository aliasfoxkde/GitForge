# Aegis Security Pattern Scan — Baseline Triage (2026-09-20)

Tool: [Aegis](https://github.com/) security scanner for DevOps and CI/CD
(`~/.local/bin/aegis`), profile `production`, repository root as the scan
target.

- **Files scanned:** 587 (31 590 skipped by gitignore/binary rules)
- **Findings:** 1 090 — 17 critical, 103 high, 308 medium, 662 low
- **True secrets / exploitable issues found:** **0**

Every finding was triaged. The complete set is recorded in
`.github/aegis-baseline.json` (the gate baseline), and `make aegis` fails
only on findings that are **not** in that baseline. This document records
why the existing findings are accepted.

## How the gate works

```bash
make aegis             # fails (exit 1) only on NEW findings vs the baseline
make aegis-report      # full human-readable report, no baseline
make aegis-baseline    # regenerate .github/aegis-baseline.json after triage
```

Baseline mechanics and limitations (verified against Aegis 0.6.1):

- Fingerprints are `pattern:path:line`, so **editing a file above a finding
  shifts line numbers and re-flags it**. After moving code around, re-run
  `make aegis` and re-commit the baseline if the same findings moved.
- The scan root is part of the path: run the gate from the repository root
  (as `make aegis` does), or the fingerprints will not match.
- The profile matters: always scan with `--config production` so the
  pattern set matches the baseline's.
- Regeneration strips the SARIF inspection ledger and per-extension stats
  from the committed file (they carried ~78 MB of per-file inspection
  records); the findings list is what the baseline needs.

## Triage by category

### `secrets` (59 findings) — none real

| Finding | Where | Verdict |
|---------|-------|---------|
| `env-credential-assignment` | `.github/workflows/ai-review.yml`, `.github/actions/templates/*.yml` | `${{ secrets.* }}` references — the correct GitHub Actions pattern, flagged for looking like a credential assignment |
| `env-credential-assignment` | `Makefile` (`run-api` target) | Local development convenience: `JWT_SECRET="dev-secret"` for `make run-api`. Documented; production deployments must set a real secret (`docs/RUNBOOK.md`) |
| `env-credential-assignment` | `.env.example`, `template-parts/*/docker-compose.yml` | Example/template files with placeholder values (`your-secret-here`, `changeme`) |
| `hardcoded-password` | `crates/gitforge-common/src/password.rs`, `crates/gitforge-api/src/routes/auth.rs`, `services/api/src/main.rs`, `crates/gitforge-api/src/middleware.rs` | `#[cfg(test)]` code: bcrypt round-trip fixtures (`"secure_password_123"`), Debug-impl assertions (`"test-secret"`) |
| `hardcoded-password` | `crates/gitforge-cli/src/main.rs` | `rpassword` **reads** passwords from the terminal; nothing is hardcoded |
| `hardcoded-password` | `crates/gitforge-review/src/{security,fix}.rs` | Literal strings of the review engine's own vulnerability detectors (pattern tables that must contain the words they match) |
| `hardcoded-password` | `config.toml.example` | Documented placeholder with an explicit "CHANGE THIS IN PRODUCTION!" comment |
| `hardcoded-username` | `supply-chain/imports.lock` | Cargo-vet registry metadata (audit registry maintainer e-mail addresses) |
| `ssh-private-key`, `github-ssh-key` | `crates/gitforge-review/src/security.rs:554` | The detector's match string (`BEGIN OPENSSH PRIVATE KEY`); there is no key material in the file |

### `pii` (573 findings, 3 critical) — none real

- `.github/ISSUE_TEMPLATE/pattern_submission.yml` — AWS's documented
  AWS-style example key IDs inside the issue-template submission used
  to *submit* new scanner patterns; examples are the point of the file.
- `crates/gitforge-api/src/metrics_middleware.rs:186` — a UUID-format test
  string matched by a Luhn-style card heuristic.
- `crates/gitforge-api/src/routes/webhook.rs` — 12-digit substrings inside
  hex webhook-signature fixtures matched by the Aadhaar heuristic.
- Bulk of the category: RFC 2606 example addresses
  (`user@example.com`) in tests, docs, and templates, plus author
  e-mail addresses in lock files.

### `web-security` (235 findings, 11 critical) — none real

- `release.yml:93` `executable-file-upload` — that is the release job
  uploading release binaries; expected by definition.
- `docs/MACOS_BUILD.md:276` XXE — XML *example text* in documentation.
- `template-parts/**` — scaffolding templates for downstream projects;
  their example endpoints/commands are intentional teaching material.
- Test harnesses in `crates/gitforge-runner/src/agent.rs`,
  `services/git-server/tests/*`, `services/api/*` bind 127.0.0.1 mock
  HTTP servers (`ssrf-localhost`) and use credential-in-URL strings to
  verify URL parsing (`password-in-url`). Both are the behavior under
  test, not vulnerabilities.

### `security-hardening` (198 findings) — hardening advice, no defects

Advice-level items (missing security headers in example responses,
XML parser guidance in docs, `basic-auth-url` in fixture URLs) recorded
in the baseline; genuine hardening work is tracked in
`docs/planning/IMPROVEMENTS.md`, not here.

### `compliance` (25 findings) — reference text

HIPAA/PCI/GDPR keyword mentions inside docs and the review engine's
pattern tables.

## Accepted deviations worth knowing about

1. `make run-api` uses `JWT_SECRET="dev-secret"` — local development
   only. The API is fail-closed: it refuses to start when `JWT_SECRET`
   is unset (`services/api/src/main.rs`, "no dev fallback in
   production"); production must set a strong secret (see
   `docs/RUNBOOK.md`).
2. The AI review workflow reads provider keys from `${{ secrets.* }}` and
   skips the review when unset — no credential is embedded anywhere.

## Follow-up

- New findings introduced by future changes fail `make aegis`; triage and
  either fix them or regenerate the baseline with a dated note in this
  document.
- The line-number sensitivity of the baseline means large refactors will
  require a baseline refresh; prefer fixing real findings over
  re-baselining repeatedly.
