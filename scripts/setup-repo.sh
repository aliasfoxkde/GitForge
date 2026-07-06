#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Dark Factory Repository Bootstrap
# Creates a new GitHub repo from the dark-factory template and configures it
#
# Usage:
#   ./scripts/setup-repo.sh <owner> <repo-name> [options]
#
# Example:
#   ./scripts/setup-repo.sh aliasfoxkde my-new-project --public
#   ./scripts/setup-repo.sh aliasfoxkde taskwizer --private --coverage-threshold=80
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

INFO() { echo -e "${GREEN}[setup]${NC} $*"; }
WARN() { echo -e "${YELLOW}[setup]${NC} $*"; }
ERROR() { echo -e "${RED}[setup]${NC} $*"; }
STEP() { echo -e "${BLUE}[step]${NC} $*"; }

usage() {
    echo "Usage: $0 <owner> <repo-name> [options]"
    echo ""
    echo "Options:"
    echo "  --public                  Create public repository (default: private)"
    echo "  --private                 Create private repository"
    echo "  --default-branch <name>   Default branch name (default: main)"
    echo "  --coverage-threshold <n>  Coverage threshold % (default: 70)"
    echo "  --go-versions <json>      JSON array of Go versions"
    echo "  --golangci-version <ver>  golangci-lint version"
    echo "  --template                Clone from dark-factory template"
    exit 1
}

# Parse arguments
OWNER="${1:-}"; shift || usage
REPO_NAME="${1:-}"; shift || usage
[ -z "$OWNER" ] || [ -z "$REPO_NAME" ] && usage

VISIBILITY="--private"
DEFAULT_BRANCH="main"
COVERAGE_THRESHOLD="70"
GO_VERSIONS='["1.21","1.22","1.23","1.24"]'
GOLANGCI_VERSION="v2.1.6"
FROM_TEMPLATE=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --public) VISIBILITY="--public"; shift ;;
        --private) VISIBILITY="--private"; shift ;;
        --default-branch) DEFAULT_BRANCH="$2"; shift 2 ;;
        --coverage-threshold) COVERAGE_THRESHOLD="$2"; shift 2 ;;
        --go-versions) GO_VERSIONS="$2"; shift 2 ;;
        --golangci-version) GOLANGCI_VERSION="$2"; shift 2 ;;
        --template) FROM_TEMPLATE=true; shift ;;
        *) ERROR "Unknown option: $1"; usage ;;
    esac
done

FULL_REPO="$OWNER/$REPO_NAME"
DESCRIPTION="${DESCRIPTION:-Dark Factory powered repository}"

# ─── Step 1: Create repository ────────────────────────────────────────────────
STEP "Creating GitHub repository: $FULL_REPO"
if $FROM_TEMPLATE; then
    gh repo create "$FULL_REPO" \
        --template "aliasfoxkde/dark-factory" \
        $VISIBILITY \
        --description "$DESCRIPTION"
else
    gh repo create "$FULL_REPO" $VISIBILITY --description "$DESCRIPTION"
fi

# ─── Step 2: Clone and push initial structure ─────────────────────────────────
STEP "Setting up repository structure..."
TMP_DIR=$(mktemp -d)
git clone "https://$(gh auth token)@github.com/$FULL_REPO.git" "$TMP_DIR"

# Copy dark-factory structure
rsync -av --exclude='.git' "$REPO_ROOT/" "$TMP_DIR/" 2>/dev/null || cp -r "$REPO_ROOT/"* "$TMP_DIR/"

cd "$TMP_DIR"
git add -A
git commit -m "feat: initial commit from Dark Factory template

Co-Authored-By: Claude <noreply@anthropic.com>"
git push -u origin "$DEFAULT_BRANCH"

cd /tmp
rm -rf "$TMP_DIR"

# ─── Step 3: Trigger setup-repo workflow ────────────────────────────────────
STEP "Running repository configuration workflow..."
gh workflow run setup-repo.yml \
    -f repo="$FULL_REPO" \
    -f owner="$OWNER" \
    -f default_branch="$DEFAULT_BRANCH" \
    -f coverage_threshold="$COVERAGE_THRESHOLD" \
    -f golangci_version="$GOLANGCI_VERSION" \
    -f go_versions="$GO_VERSIONS"

# ─── Step 4: Configure Dependabot ────────────────────────────────────────────
STEP "Configuring Dependabot..."
gh api repos/$FULL_REPO/variables --method POST \
    -f name="DEPENDABOT_SCHEDULE" -f value="weekly" 2>/dev/null || true

# ─── Step 5: Summary ─────────────────────────────────────────────────────────
INFO ""
INFO "══════════════════════════════════════════════════════════"
INFO "  Dark Factory repository setup complete!"
INFO "══════════════════════════════════════════════════════════"
INFO "  Repository:  https://github.com/$FULL_REPO"
INFO "  Default branch: $DEFAULT_BRANCH"
INFO "  Coverage threshold: ${COVERAGE_THRESHOLD}%"
INFO "  Go versions: $GO_VERSIONS"
INFO ""
INFO "  Next steps:"
INFO "  1. Customize .github/CODEOWNERS (replace aliasfoxkde)"
INFO "  2. Customize .github/PULL_REQUEST_TEMPLATE.md"
INFO "  3. Customize .github/wiki/ content"
INFO "  4. Customize template-parts/ for your stack"
INFO "  5. Run: gh repo clone $FULL_REPO"
INFO ""
