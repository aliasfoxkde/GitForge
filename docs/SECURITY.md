# Security Policy

## Supported Versions

Only the latest release is supported with security updates.

| Version | Supported          |
|---------|-------------------|
| latest  | ✅ Supported       |
| < latest| ❌ Not supported  |

## Reporting a Vulnerability

We take security seriously. Please report vulnerabilities responsibly.

### How to Report

1. **Private reporting (preferred):** Go to the repository's Security tab →
   " Advisories" → "Report a vulnerability"

2. **Email:** Contact the maintainer directly with a description of the
   vulnerability and steps to reproduce

### Response Timeline

- **72 hours:** Acknowledge receipt of your report
- **7 days:** Initial assessment of severity and impact
- **30 days:** Resolution or proposed timeline for fix

### Scope

We are most interested in:
- False negatives in security patterns
- ReDoS vulnerabilities in pattern matching
- Malicious pattern injection
- Secrets leakage via pattern matches
- Denial of service via crafted inputs

### Out of Scope

- Social engineering
- Physical security
- DDoS attacks on our infrastructure