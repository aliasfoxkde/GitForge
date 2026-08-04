#!/bin/bash
# ci_validation.sh - CI environment detection and feature flags
# Part of Dark Factory code-library snippets

set -euo pipefail

# Detect CI environment
is_ci() {
    [[ "${CI:-false}" == "true" ]]
}

# Detect specific CI providers
is_github_actions() {
    [[ "${GITHUB_ACTIONS:-false}" == "true" ]]
}

is_gitlab_ci() {
    [[ "${GITLAB_CI:-false}" == "true" ]]
}

is_circle_ci() {
    [[ "${CIRCLECI:-false}" == "true" ]]
}

is_jenkins() {
    [[ "${JENKINS_HOME:-false}" != "false" ]]
}

is_travis() {
    [[ "${TRAVIS:-false}" == "true" ]]
}

# Get CI provider name
ci_provider() {
    if is_github_actions; then
        echo "github-actions"
    elif is_gitlab_ci; then
        echo "gitlab-ci"
    elif is_circle_ci; then
        echo "circleci"
    elif is_jenkins; then
        echo "jenkins"
    elif is_travis; then
        echo "travis"
    elif is_ci; then
        echo "unknown-ci"
    else
        echo "local"
    fi
}

# Detect if we're in a pull request
is_pr() {
    if is_github_actions; then
        [[ "${GITHUB_EVENT_NAME:-}" == "pull_request" ]]
    elif is_gitlab_ci; then
        [[ "${CI_MERGE_REQUEST_IID:-}" != "" ]]
    else
        [[ -n "${PR:-}" ]]
    fi
}

# Get branch name
branch_name() {
    if is_github_actions; then
        echo "${GITHUB_REF_NAME:-$(git rev-parse --abbrev-ref HEAD)}"
    elif is_gitlab_ci; then
        echo "${CI_COMMIT_REF_NAME:-$(git rev-parse --abbrev-ref HEAD)}"
    elif is_circle_ci; then
        echo "${CIRCLE_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}"
    else
        git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown"
    fi
}

# Get commit SHA
commit_sha() {
    if is_github_actions; then
        echo "${GITHUB_SHA:-}"
    elif is_gitlab_ci; then
        echo "${CI_COMMIT_SHA:-}"
    elif is_circle_ci; then
        echo "${CIRCLE_SHA1:-}"
    else
        git rev-parse HEAD 2>/dev/null || echo ""
    fi
}

# Detect if running on specific OS
is_linux() {
    [[ "${OSTYPE}" == "linux-gnu"* ]]
}

is_macos() {
    [[ "${OSTYPE}" == "darwin"* ]]
}

is_windows() {
    [[ "${OSTYPE}" == "cygwin" ]] || [[ "${OSTYPE}" == "msys" ]] || [[ "${OSTYPE}" == "win32" ]]
}

# Get OS name
os_name() {
    if is_linux; then
        echo "linux"
    elif is_macos; then
        echo "macos"
    elif is_windows; then
        echo "windows"
    else
        echo "unknown"
    fi
}

# Feature flags based on CI environment
is_feature_enabled() {
    local feature="$1"
    case "$feature" in
        strict-linting)
            is_ci
            ;;
        coverage-enforcement)
            is_ci
            ;;
        parallel-tests)
            is_ci
            ;;
        full-test-suite)
            is_pr || is_ci
            ;;
        security-scan)
            is_github_actions
            ;;
        *)
            false
            ;;
    esac
}

# Get number of CPU cores (for parallelization)
cpu_cores() {
    local cores
    if is_macos; then
        cores=$(sysctl -n hw.ncpu 2>/dev/null || echo 1)
    elif is_linux; then
        cores=$(nproc 2>/dev/null || echo 1)
    else
        cores=1
    fi
    echo "${CORES:-$cores}"
}

# Get recommended test parallelism
test_parallelism() {
    local cores=$(cpu_cores)
    # Leave 1 core for system, use rest for tests
    local parallelism=$((cores > 1 ? cores - 1 : 1))
    echo "${TEST_PARALLELISM:-$parallelism}"
}

# Environment summary (for debugging)
env_summary() {
    echo "=== CI Environment ==="
    echo "Provider: $(ci_provider)"
    echo "CI detected: $(is_ci && echo yes || echo no)"
    echo "PR detected: $(is_pr && echo yes || echo no)"
    echo "Branch: $(branch_name)"
    echo "Commit: $(commit_sha)"
    echo "OS: $(os_name)"
    echo "CPU cores: $(cpu_cores)"
    echo "Test parallelism: $(test_parallelism)"
    echo "======================"
}

# Main entry point
main() {
    local command="${1:-summary}"

    case "$command" in
        provider)
            ci_provider
            ;;
        is-ci)
            is_ci && exit 0 || exit 1
            ;;
        is-pr)
            is_pr && exit 0 || exit 1
            ;;
        branch)
            branch_name
            ;;
        sha)
            commit_sha
            ;;
        os)
            os_name
            ;;
        cores)
            cpu_cores
            ;;
        parallelism)
            test_parallelism
            ;;
        summary)
            env_summary
            ;;
        feature)
            is_feature_enabled "${2:-}" && exit 0 || exit 1
            ;;
        *)
            echo "Usage: $0 {provider|is-ci|is-pr|branch|sha|os|cores|parallelism|summary|feature} [feature-name]"
            exit 1
            ;;
    esac
}

main "$@"
