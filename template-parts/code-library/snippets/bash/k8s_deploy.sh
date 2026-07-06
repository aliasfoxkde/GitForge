#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Kubernetes deployment script with rollback support
# Features: deployment, rollback, health checks, resource cleanup
# ─────────────────────────────────────────────────────────────────────────────
set -Eeuo pipefail

# ─── Configuration ────────────────────────────────────────────────────────────
readonly SCRIPT_NAME="$(basename "$0")"
readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_VERSION="1.0.0"

# Kubernetes config
KUBECTL="${KUBECTL:-kubectl}"
NAMESPACE="${NAMESPACE:-default}"
DEPLOYMENT_TIMEOUT="${DEPLOYMENT_TIMEOUT:-300}"
HEALTH_CHECK_RETRIES="${HEALTH_CHECK_RETRIES:-30}"
HEALTH_CHECK_INTERVAL="${HEALTH_CHECK_INTERVAL:-10}"

# ─── Colors ───────────────────────────────────────────────────────────────────
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly NC='\033[0m'

# ─── Logging ───────────────────────────────────────────────────────────────────
log_info()   { echo -e "${GREEN}[INFO]${NC} $*"; }
log_warn()   { echo -e "${YELLOW}[WARN]${NC} $*" >&2; }
log_error()  { echo -e "${RED}[ERROR]${NC} $*" >&2; }
log_debug()  { [[ "${DEBUG:-0}" == "1" ]] && echo -e "${BLUE}[DEBUG]${NC} $*" || true; }

# ─── Usage ─────────────────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Usage: $SCRIPT_NAME <command> [options]

Commands:
    deploy       Deploy a Kubernetes manifest
    rollback     Rollback to previous revision
    status       Check deployment status
    logs         View pod logs
    cleanup      Remove resources

Options:
    -n, --namespace NS    Kubernetes namespace (default: $NAMESPACE)
    -f, --file FILE       Manifest file or directory
    -d, --deployment NAME Deployment name
    -r, --revision REV    Revision to rollback to
    -h, --help            Show this help

Examples:
    $SCRIPT_NAME deploy -f k8s/deployment.yaml
    $SCRIPT_NAME rollback -d myapp -r 2
    $SCRIPT_NAME status -d myapp
EOF
}

# ─── Helpers ───────────────────────────────────────────────────────────────────
require() {
    local cmd="$1"; local msg="${2:-}"
    if ! command -v "$cmd" &>/dev/null; then
        log_error "Required: $cmd${msg:+, $msg}"
        exit 1
    fi
}

get_revisions() {
    local deployment="$1"
    "$KUBECTL" rollout history deployment/"$deployment" -n "$NAMESPACE" \
        2>/dev/null | grep -E "^[0-9]+" | awk '{print $1}' | sort -n
}

get_current_revision() {
    local deployment="$1"
    "$KUBECTL" rollout history deployment/"$deployment" -n "$NAMESPACE" \
        2>/dev/null | grep -E "^${deployment}" | awk '{print $2}' | tr -d '#'
}

# ─── Commands ─────────────────────────────────────────────────────────────────

cmd_deploy() {
    local file="" deployment=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -f|--file) file="$2"; shift 2 ;;
            -d|--deployment) deployment="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    [[ -z "$file" ]] && { log_error "Manifest file required (-f)"; return 1; }
    require "$KUBECTL"

    log_info "Deploying from: $file"

    # Apply manifest
    "$KUBECTL" apply -f "$file" -n "$NAMESPACE"

    # Extract deployment name if not provided
    if [[ -z "$deployment" ]]; then
        deployment=$("$KUBECTL" get -f "$file" -n "$NAMESPACE" \
            -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)
    fi

    if [[ -n "$deployment" ]]; then
        log_info "Waiting for rollout to complete: $deployment"

        if ! "$KUBECTL" rollout status deployment/"$deployment" \
            -n "$NAMESPACE" --timeout="${DEPLOYMENT_TIMEOUT}s"; then
            log_error "Rollout failed"
            log_info "Showing recent events:"
            "$KUBECTL" get events -n "$NAMESPACE" \
                --sort-by='.lastTimestamp' | tail -10
            return 1
        fi

        log_info "Deployment successful"
        log_deployment_info "$deployment"
    fi
}

log_deployment_info() {
    local deployment="$1"
    log_info "Deployment: $deployment"
    log_info "Replicas: $("$KUBECTL" get deployment "$deployment" -n "$NAMESPACE" \
        -o jsonpath='{.status.replicas}{"/"}{.status.readyReplicas}{"/"}{.spec.replicas}')"
    log_info "Image: $("$KUBECTL" get deployment "$deployment" -n "$NAMESPACE" \
        -o jsonpath='{.spec.template.spec.containers[0].image}')"
}

cmd_rollback() {
    local deployment="" revision=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -d|--deployment) deployment="$2"; shift 2 ;;
            -r|--revision) revision="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    [[ -z "$deployment" ]] && { log_error "Deployment name required (-d)"; return 1; }
    require "$KUBECTL"

    local current_rev
    current_rev=$(get_current_revision "$deployment")
    log_info "Current revision: $current_rev"

    if [[ -z "$revision" ]]; then
        log_info "Available revisions:"
        get_revisions "$deployment" | tail -5
        log_warn "Specify revision with -r"
        return 1
    fi

    log_info "Rolling back to revision: $revision"
    "$KUBECTL" rollout undo deployment/"$deployment" \
        -n "$NAMESPACE" --to-revision="$revision"

    log_info "Waiting for rollback to complete..."
    "$KUBECTL" rollout status deployment/"$deployment" \
        -n "$NAMESPACE" --timeout="${DEPLOYMENT_TIMEOUT}s"

    log_info "Rollback complete"
}

cmd_status() {
    local deployment=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -d|--deployment) deployment="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    require "$KUBECTL"

    if [[ -z "$deployment" ]]; then
        log_info "All deployments in namespace: $NAMESPACE"
        "$KUBECTL" get deployments -n "$NAMESPACE" \
            -o=custom-columns='NAME:.metadata.name,READY:.status.readyReplicas,AGE:.metadata.creationTimestamp'
        return
    fi

    log_deployment_info "$deployment"

    log_info "Pods:"
    "$KUBECTL" get pods -n "$NAMESPACE" -l "app=$deployment" \
        -o=custom-columns='NAME:.metadata.name,STATUS:.status.phase,READY:.status.readyReplicas,RESTARTS:.status.containerStatuses[0].restartCount'

    local ready
    ready=$("$KUBECTL" get deployment "$deployment" -n "$NAMESPACE" \
        -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo "0")
    local desired
    desired=$("$KUBECTL" get deployment "$deployment" -n "$NAMESPACE" \
        -o jsonpath='{.spec.replicas}' 2>/dev/null || echo "0")

    if [[ "$ready" == "$desired" ]]; then
        log_info "Status: ${GREEN}Healthy${NC}"
    else
        log_error "Status: ${RED}Unhealthy${NC} ($ready/$desired ready)"
    fi
}

cmd_logs() {
    local deployment="" container="" tail=100

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -d|--deployment) deployment="$2"; shift 2 ;;
            -c|--container) container="$2"; shift 2 ;;
            --tail) tail="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    [[ -z "$deployment" ]] && { log_error "Deployment name required (-d)"; return 1; }
    require "$KUBECTL"

    local pod
    pod=$("$KUBECTL" get pods -n "$NAMESPACE" -l "app=$deployment" \
        -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)

    [[ -z "$pod" ]] && { log_error "No pods found for: $deployment"; return 1; }

    log_info "Showing logs for: $pod"

    local kubectl_args=("logs" "$pod" "-n" "$NAMESPACE" "--tail=$tail")
    [[ -n "$container" ]] && kubectl_args+=("-c" "$container")

    "${kubectl_args[@]}" || true
}

cmd_cleanup() {
    local file="" deployment=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -f|--file) file="$2"; shift 2 ;;
            -d|--deployment) deployment="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    require "$KUBECTL"

    if [[ -n "$file" ]]; then
        log_warn "Removing resources from: $file"
        "$KUBECTL" delete -f "$file" -n "$NAMESPACE" --ignore-not-found
    elif [[ -n "$deployment" ]]; then
        log_warn "Removing deployment: $deployment"
        "$KUBECTL" delete deployment "$deployment" -n "$NAMESPACE" --ignore-not-found
        "$KUBECTL" delete svc "-l" "app=$deployment" -n "$NAMESPACE" --ignore-not-found
    else
        log_error "Specify file (-f) or deployment (-d)"
        return 1
    fi

    log_info "Cleanup complete"
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    [[ $# -lt 1 ]] && { usage; exit 1; }

    local command="$1"; shift

    # Parse global options
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -n|--namespace) NAMESPACE="$2"; shift 2 ;;
            -h|--help) usage; exit 0 ;;
            -*) shift ;;
            *) break ;;
        esac
    done

    case "$command" in
        deploy)    cmd_deploy "$@" ;;
        rollback)  cmd_rollback "$@" ;;
        status)    cmd_status "$@" ;;
        logs)      cmd_logs "$@" ;;
        cleanup)   cmd_cleanup "$@" ;;
        help)      usage ;;
        *)         log_error "Unknown command: $command"; usage; exit 1 ;;
    esac
}

main "$@"
