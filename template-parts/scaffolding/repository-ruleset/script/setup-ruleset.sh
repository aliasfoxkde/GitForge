#!/bin/bash
# setup-ruleset.sh - Configure repository guardrails (rulesets)
# Imports Safeguards from Atheon-Enhanced patterns
# Part of dark-factory template system

set -e

REPO="${1:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"
AUTH_TOKEN="$(gh auth token)"

echo "🔧 Setting up repository guardrails: $REPO"

# Check if ruleset already exists
EXISTING_RULESET=$(gh api repos/"$REPO"/rulesets --jq '.[] | select(.name == "Safeguards") | .id' 2>/dev/null || echo "")

if [ -n "$EXISTING_RULESET" ]; then
    echo "⚠️  Safeguards ruleset already exists (ID: $EXISTING_RULESET)"
    echo "   Delete it first to recreate: gh api repos/$REPO/rulesets/$EXISTING_RULESET -X DELETE"
    exit 0
fi

# Create the Safeguards ruleset via curl (gh api --input doesn't support complex JSON)
echo "📋 Creating Safeguards ruleset..."
curl -s -X POST "https://api.github.com/repos/$REPO/rulesets" \
    -H "Accept: application/vnd.github+json" \
    -H "Authorization: Bearer $AUTH_TOKEN" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    -d '{
  "name": "Safeguards",
  "target": "branch",
  "enforcement": "active",
  "conditions": {
    "ref_name": {
      "exclude": ["refs/heads/stable/clean"],
      "include": ["~DEFAULT_BRANCH", "refs/heads/stable/*", "refs/heads/release/*", "refs/heads/feature/*"]
    }
  },
  "rules": [
    {"type": "deletion"},
    {"type": "non_fast_forward"},
    {"type": "required_linear_history"},
    {"type": "pull_request", "parameters": {"required_approving_review_count": 0, "dismiss_stale_reviews_on_push": true, "required_reviewers": [], "require_code_owner_review": false, "require_last_push_approval": false, "required_review_thread_resolution": true, "allowed_merge_methods": ["merge", "squash", "rebase"]}}
  ]
}' > /dev/null

echo "✅ Safeguards ruleset created"
echo ""
echo "📊 Current rulesets:"
gh api repos/"$REPO"/rulesets --jq '.[] | "  - \(.name) (\(.enforcement))"' 2>/dev/null || echo "  None"