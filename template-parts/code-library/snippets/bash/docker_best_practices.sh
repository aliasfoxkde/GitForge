#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Docker build/push script with multi-stage builds and no-cache tags
# Features: multi-stage builds, layer caching, image tagging, digest tracking
# ─────────────────────────────────────────────────────────────────────────────
set -Eeuo pipefail

# ─── Configuration ────────────────────────────────────────────────────────────
readonly SCRIPT_NAME="$(basename "$0")"
readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_VERSION="1.0.0"

# Docker config
DOCKER="${DOCKER:-docker}"
REGISTRY="${REGISTRY:-docker.io}"
IMAGE_NAME="${IMAGE_NAME:-}"
TAG_PREFIX="${TAG_PREFIX:-}"

# Build config
DOCKERFILE="${DOCKERFILE:-Dockerfile}"
BUILD_CONTEXT="${BUILD_CONTEXT:-.}"
BUILD_ARGS=()
NO_CACHE="${NO_CACHE:-false}"

# ─── Colors ───────────────────────────────────────────────────────────────────
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly CYAN='\033[0;36m'
readonly NC='\033[0m'

# ─── Logging ───────────────────────────────────────────────────────────────────
log_info()   { echo -e "${GREEN}[docker]${NC} $*"; }
log_warn()   { echo -e "${YELLOW}[docker]${NC} $*" >&2; }
log_error()  { echo -e "${RED}[docker]${NC} $*" >&2; }
log_debug()  { [[ "${DEBUG:-0}" == "1" ]] && echo -e "${BLUE}[docker]${NC} $*" || true; }

# ─── Usage ─────────────────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Usage: $SCRIPT_NAME <command> [options]

Commands:
    build       Build Docker image
    push        Push image to registry
    build-push  Build and push in one step
    inspect     Show image info
    clean       Remove local images

Options:
    -n, --name NAME         Image name (required)
    -t, --tag TAG           Image tag (default: latest)
    -f, --file DOCKERFILE   Dockerfile path (default: ./Dockerfile)
    -r, --registry REG      Registry URL (default: docker.io)
    --no-cache              Build without layer cache
    --build-arg KEY=VAL     Build arguments

Examples:
    $SCRIPT_NAME build -n myapp -t v1.0.0
    $SCRIPT_NAME build-push -n myapp -t v1.0.0 --no-cache
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

get_image_digest() {
    local image="$1"
    "$DOCKER" inspect --format='{{index .RepoDigests 0}}' "$image" 2>/dev/null \
        | cut -d@ -f2 || echo "unknown"
}

get_image_size() {
    local image="$1"
    "$DOCKER" inspect --format='{{.Size}}' "$image" 2>/dev/null \
        | numfmt --to=iec-i1024 --suffix=B || echo "unknown"
}

format_image() {
    local image="$1"; local tag="${2:-latest}"
    if [[ "$image" == *"/"* ]]; then
        echo "${image}:${tag}"
    else
        echo "${REGISTRY}/${image}:${tag}"
    fi
}

parse_build_args() {
    local args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --build-arg)
                args+=("--build-arg" "$2")
                shift 2
                ;;
            *)
                shift
                ;;
        esac
    done
    echo "${args[@]}"
}

# ─── Commands ─────────────────────────────────────────────────────────────────

cmd_build() {
    local name="" tag="latest" dockerfile="$DOCKERFILE"
    local build_args=()

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -n|--name) name="$2"; shift 2 ;;
            -t|--tag) tag="$2"; shift 2 ;;
            -f|--file) dockerfile="$2"; shift 2 ;;
            --build-arg) build_args+=("--build-arg" "$2"); shift 2 ;;
            --no-cache) NO_CACHE="true"; shift ;;
            *) shift ;;
        esac
    done

    [[ -z "$name" ]] && { log_error "Image name required (-n)"; return 1; }
    require "$DOCKER" "https://docs.docker.com/get-docker/"

    local full_image
    full_image=$(format_image "$name" "$tag")

    log_info "Building image: $full_image"
    log_info "Dockerfile: $dockerfile"
    log_info "Context: $BUILD_CONTEXT"

    # Build command
    local build_cmd=("$DOCKER" build)

    # Add no-cache if requested
    if [[ "$NO_CACHE" == "true" ]]; then
        build_cmd+=("--no-cache")
        log_warn "Building without layer cache"
    fi

    # Add build args
    for arg in "${build_args[@]}"; do
        build_cmd+=("--build-arg" "$arg")
    done

    # Add common flags
    build_cmd+=(
        --file "$dockerfile"
        --tag "$full_image"
        --progress=plain
    )

    # Add context
    build_cmd+=("$BUILD_CONTEXT")

    log_debug "Command: ${build_cmd[*]}"

    if ! "${build_cmd[@]}"; then
        log_error "Build failed"
        return 1
    fi

    log_info "Build complete"
    log_info "Size: $(get_image_size "$full_image")"

    # Show image info
    log_info "Layers: $("$DOCKER" history "$full_image" --no-trunc | wc -l)"
}

cmd_push() {
    local name="" tag="latest"

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -n|--name) name="$2"; shift 2 ;;
            -t|--tag) tag="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    [[ -z "$name" ]] && { log_error "Image name required (-n)"; return 1; }
    require "$DOCKER"

    local full_image
    full_image=$(format_image "$name" "$tag")

    log_info "Pushing image: $full_image"

    if ! "$DOCKER" push "$full_image"; then
        log_error "Push failed"
        return 1
    fi

    # Record digest
    local digest
    digest=$(get_image_digest "$full_image")
    log_info "Digest: $digest"
    log_info "Push complete"
}

cmd_build_push() {
    local name="" tag="latest"

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -n|--name) name="$2"; shift 2 ;;
            -t|--tag) tag="$2"; shift 2 ;;
            --no-cache) NO_CACHE="true"; shift ;;
            --build-arg) build_args+=("--build-arg" "$2"); shift 2 ;;
            *) shift ;;
        esac
    done

    [[ -z "$name" ]] && { log_error "Image name required (-n)"; return 1; }

    log_info "Build and push: $name:$tag"

    # Build
    cmd_build "$@"
    local build_status=$?

    if [[ $build_status -ne 0 ]]; then
        log_error "Build failed, skipping push"
        return 1
    fi

    # Push
    cmd_push -n "$name" -t "$tag"
}

cmd_inspect() {
    local name="" tag="latest"

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -n|--name) name="$2"; shift 2 ;;
            -t|--tag) tag="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    [[ -z "$name" ]] && { log_error "Image name required (-n)"; return 1; }

    local full_image
    full_image=$(format_image "$name" "$tag")

    require "$DOCKER"

    if ! "$DOCKER" image inspect "$full_image" &>/dev/null; then
        log_error "Image not found: $full_image"
        return 1
    fi

    log_info "Image: $full_image"
    log_info "Size: $(get_image_size "$full_image")"
    log_info "Digest: $(get_image_digest "$full_image")"
    log_info "Created: $($DOCKER" inspect --format='{{.Created}}' "$full_image")"

    log_info "Environment:"
    "$DOCKER" inspect --format='{{range .Config.Env}}{{println .}}{{end}}' "$full_image" 2>/dev/null

    log_info "Ports: $($DOCKER" inspect --format='{{range .ExposedPorts}}{{.}} {{end}}' "$full_image" 2>/dev/null || echo "none")"
}

cmd_clean() {
    local name=""; local tag="latest"; local force=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -n|--name) name="$2"; shift 2 ;;
            -t|--tag) tag="$2"; shift 2 ;;
            -f|--force) force="--force"; shift ;;
            *) shift ;;
        esac
    done

    require "$DOCKER"

    if [[ -z "$name" ]]; then
        log_error "Image name required (-n)"
        return 1
    fi

    local full_image
    full_image=$(format_image "$name" "$tag")

    log_warn "Removing image: $full_image"
    "$DOCKER" rmi "$full_image" $force 2>/dev/null || true
    log_info "Cleanup complete"
}

# ─── Main ─────────────────────────────────────────────────────────────────────
main() {
    [[ $# -lt 1 ]] && { usage; exit 1; }

    local command="$1"; shift

    case "$command" in
        build)      cmd_build "$@" ;;
        push)       cmd_push "$@" ;;
        build-push) cmd_build_push "$@" ;;
        inspect)    cmd_inspect "$@" ;;
        clean)      cmd_clean "$@" ;;
        help)       usage ;;
        *)          log_error "Unknown command: $command"; usage; exit 1 ;;
    esac
}

main "$@"
