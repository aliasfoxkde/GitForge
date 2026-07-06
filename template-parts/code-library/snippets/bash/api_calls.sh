#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Robust API call patterns for bash
# Features: retry with backoff, timeout, error handling, rate limiting
# ─────────────────────────────────────────────────────────────────────────────
set -Eeuo pipefail

# ─── Configuration ────────────────────────────────────────────────────────────
API_TIMEOUT="${API_TIMEOUT:-30}"       # seconds per request
API_MAX_RETRIES="${API_MAX_RETRIES:-3}" # number of retries
API_BACKOFF="${API_BACKOFF:-2}"         # backoff multiplier (seconds)
API_KEY="${API_KEY:-}"                   # API key (set via environment)

# ─── Colors ───────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'

# ─── Logging ───────────────────────────────────────────────────────────────────
log_info()  { echo -e "${GREEN}[api]${NC} $*"; }
log_warn()  { echo -e "${YELLOW}[api]${NC} $*" >&2; }
log_error() { echo -e "${RED}[api]${NC} $*" >&2; }

# ─── HTTP Methods ──────────────────────────────────────────────────────────────
api_get() {
    local url="$1"; shift
    local headers=("${@:-}")"

    _api_request "GET" "$url" "" "${headers[@]}"
}

api_post() {
    local url="$1"; local body="$2"; shift
    local headers=("${@:-}")

    _api_request "POST" "$url" "$body" "${headers[@]}"
}

api_put() {
    local url="$1"; local body="$2"; shift
    local headers=("${@:-}")

    _api_request "PUT" "$url" "$body" "${headers[@]}"
}

api_delete() {
    local url="$1"; shift
    local headers=("${@:-}")

    _api_request "DELETE" "$url" "" "${headers[@]}"
}

# ─── Core Request Function ─────────────────────────────────────────────────────
_api_request() {
    local method="$1"; local url="$2"; local body="$3"; shift 3
    local headers=("$@")

    local attempt=0
    local status_code=

    while (( attempt < API_MAX_RETRIES )); do
        ((attempt++))
        log_debug "$method $url (attempt $attempt/$API_MAX_RETRIES)"

        # Build curl command
        local curl_args=(
            -sS
            -w "\n%{http_code}"
            -X "$method"
            --max-time "$API_TIMEOUT"
            --location
        )

        # Add headers
        curl_args+=(-H "Content-Type: application/json")
        curl_args+=(-H "Accept: application/json")
        [[ -n "$API_KEY" ]] && curl_args+=(-H "Authorization: Bearer $API_KEY")
        for header in "${headers[@]:-}"; do
            [[ -n "$header" ]] && curl_args+=(-H "$header")
        done

        # Add body for POST/PUT
        if [[ -n "$body" && ("$method" == "POST" || "$method" == "PUT") ]]; then
            curl_args+=(-d "$body")
        fi

        curl_args+=("$url")

        # Execute request
        local response
        response=$("${curl_args[@]}" 2>&1) || {
            local curl_exit=$?
            log_warn "curl exited with $curl_exit"
            if (( attempt < API_MAX_RETRIES )); then
                _api_sleep $attempt
                continue
            fi
            log_error "Failed after $API_MAX_RETRIES attempts"
            return $curl_exit
        }

        # Parse status code (last line)
        status_code=$(tail -1 <<< "$response")
        local body_lines=$(($(wc -l <<< "$response") - 1))
        local response_body=$(head -"$body_lines" <<< "$response")

        # Success
        if [[ "$status_code" =~ ^2[0-9][0-9]$ ]]; then
            echo "$response_body"
            return 0
        fi

        # Retry on 429 (rate limit) or 5xx
        if [[ "$status_code" == "429" || "$status_code" =~ ^5[0-9][0-9]$ ]]; then
            log_warn "HTTP $status_code — retrying..."
            local retry_after
            retry_after=$(grep -i "retry-after" <<< "$(tail -5 <<< "$response")" | cut -d: -f2 | tr -d ' ' || echo "5")
            _api_sleep "${retry_after:-$((attempt * API_BACKOFF))}"
            continue
        fi

        # 4xx without retry — return error
        log_error "HTTP $status_code: $response_body"
        return 1
    done

    log_error "Max retries exceeded"
    return 1
}

# ─── Utility Functions ─────────────────────────────────────────────────────────
_api_sleep() {
    local attempt="${1:-1}"
    local delay=$((attempt * API_BACKOFF))
    # Cap at 60 seconds
    (( delay > 60 )) && delay=60
    sleep "$delay"
}

log_debug() {
    [[ "${DEBUG:-0}" == "1" ]] && echo -e "${BLUE}[debug]${NC} $*" || true
}

# ─── Usage Example ─────────────────────────────────────────────────────────────
# api_get "https://api.example.com/users"
# api_post "https://api.example.com/users" '{"name":"Alice","email":"alice@example.com"}'
