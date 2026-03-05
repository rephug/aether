#!/usr/bin/env bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

AETHER_BIN_DEFAULT="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}/release/aetherd"
AETHER_QUERY_BIN_DEFAULT="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}/release/aether-query"
AETHER_BIN="${AETHER_BIN:-${AETHER_BIN_DEFAULT}}"
AETHER_QUERY_BIN="${AETHER_QUERY_BIN:-${AETHER_QUERY_BIN_DEFAULT}}"

MEASURE_EXIT_CODE=0
MEASURE_WALL_MS=0
MEASURE_PEAK_RSS_KB=0
MEASURE_STDOUT_FILE=""
MEASURE_STDERR_FILE=""
MEASURE_TIME_FILE=""

FSCK_STATUS="unknown"
FSCK_ISSUE_COUNT=0
FSCK_MISSING_VECTORS=0
FSCK_MISSING_GRAPH_NODES=0
FSCK_PHANTOM_GRAPH_NODES=0
FSCK_DANGLING_EDGES=0
FSCK_ORPHANED_VECTORS=0
FSCK_INCOMPLETE_INTENTS=0

STATUS_TOTAL_SYMBOLS=0
STATUS_SYMBOLS_WITH_SIR=0
STATUS_SIR_PERCENTAGE="0.0"

HEALTH_TOTAL_EDGES=0

QUERY_BENCH_LATENCY_MS=0
QUERY_BENCH_EXIT_CODE=0
QUERY_BENCH_RESPONSE_FILE=""

CLONE_RESOLVED_SHA=""

RECOVERY_WALL_MS=0
RECOVERY_EXIT_CODE=0
RECOVERY_REPLAYED_INTENTS=0

log_info() {
    printf '[INFO] %s\n' "$*" >&2
}

log_warn() {
    printf '[WARN] %s\n' "$*" >&2
}

log_error() {
    printf '[ERROR] %s\n' "$*" >&2
}

json_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e ':a;N;$!ba;s/\n/\\n/g'
}

generate_json_result() {
    local first=1
    local key
    local value
    local escaped

    printf '{'
    for key in "$@"; do
        value="${key#*=}"
        key="${key%%=*}"
        escaped="$(json_escape "${value}")"
        if [ "${first}" -eq 0 ]; then
            printf ','
        fi
        first=0
        printf '"%s":"%s"' "${key}" "${escaped}"
    done
    printf '}'
}

percentile_from_file() {
    local file_path="$1"
    local percentile="$2"

    if [ ! -s "${file_path}" ]; then
        printf 'n/a'
        return 0
    fi

    local count
    count="$(wc -l < "${file_path}" | tr -d '[:space:]')"
    if [ -z "${count}" ] || [ "${count}" -eq 0 ]; then
        printf 'n/a'
        return 0
    fi

    local rank
    rank="$(( (percentile * count + 99) / 100 ))"
    if [ "${rank}" -lt 1 ]; then
        rank=1
    fi
    if [ "${rank}" -gt "${count}" ]; then
        rank="${count}"
    fi

    sort -n "${file_path}" | awk -v target="${rank}" 'NR == target { printf "%.3f", $1; exit }'
}

clone_repo() {
    local name="$1"
    local url="$2"
    local commit="$3"
    local dest_dir="$4"

    if [ -d "${dest_dir}/.git" ]; then
        log_info "Repo ${name} already present at ${dest_dir}; skipping clone"
        if [ "${commit}" != "HEAD" ]; then
            git -C "${dest_dir}" checkout --quiet "${commit}"
        fi
        CLONE_RESOLVED_SHA="$(git -C "${dest_dir}" rev-parse HEAD)"
        return 0
    fi

    log_info "Cloning ${name} from ${url}"
    git clone --filter=blob:none "${url}" "${dest_dir}"

    if [ "${commit}" != "HEAD" ]; then
        git -C "${dest_dir}" checkout --quiet "${commit}"
    fi

    CLONE_RESOLVED_SHA="$(git -C "${dest_dir}" rev-parse HEAD)"
    return 0
}

build_aether() {
    AETHER_BIN="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}/release/aetherd"
    if [ -x "${AETHER_BIN}" ]; then
        log_info "Using existing aetherd binary: ${AETHER_BIN}"
        return 0
    fi

    log_info "Building aetherd (--release)"
    cargo build -p aetherd --release
}

build_aether_query() {
    AETHER_QUERY_BIN="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}/release/aether-query"
    if [ -x "${AETHER_QUERY_BIN}" ]; then
        return 0
    fi

    log_info "Building aether-query (--release)"
    cargo build -p aether-query --release
}

measure_command() {
    if [ "$#" -lt 3 ]; then
        log_error "measure_command requires at least 3 arguments: label output_prefix command..."
        return 2
    fi

    local label="$1"
    local output_prefix="$2"
    shift 2

    local stdout_file="${output_prefix}.stdout.log"
    local stderr_file="${output_prefix}.stderr.log"
    local time_file="${output_prefix}.time.log"
    local start_ns
    local end_ns

    mkdir -p "$(dirname "${output_prefix}")"

    start_ns="$(date +%s%N)"
    set +e
    /usr/bin/time -v -o "${time_file}" "$@" >"${stdout_file}" 2>"${stderr_file}"
    local exit_code=$?
    set -e
    end_ns="$(date +%s%N)"

    local wall_ms
    wall_ms="$(awk -v start="${start_ns}" -v end="${end_ns}" 'BEGIN { printf "%.3f", (end - start) / 1000000 }')"

    local peak_rss_kb
    peak_rss_kb="$((
        $(awk -F: '/Maximum resident set size \(kbytes\)/ { gsub(/^[ \t]+/, "", $2); print $2; exit }' "${time_file}" 2>/dev/null || echo 0)
    ))"

    MEASURE_EXIT_CODE="${exit_code}"
    MEASURE_WALL_MS="${wall_ms}"
    MEASURE_PEAK_RSS_KB="${peak_rss_kb:-0}"
    MEASURE_STDOUT_FILE="${stdout_file}"
    MEASURE_STDERR_FILE="${stderr_file}"
    MEASURE_TIME_FILE="${time_file}"

    if [ "${exit_code}" -ne 0 ]; then
        log_warn "${label} exited with code ${exit_code}"
    fi

    return 0
}

run_aether_index() {
    local workspace="$1"
    shift
    "${AETHER_BIN}" --workspace "${workspace}" --index-once "$@"
}

run_aether_full_index() {
    local workspace="$1"
    shift
    "${AETHER_BIN}" --workspace "${workspace}" --index-once --full "$@"
}

_extract_metric_count() {
    local label="$1"
    local file_path="$2"
    local value

    value="$(awk -F: -v key="${label}" '$0 ~ key { gsub(/[^0-9]/, "", $2); print $2; exit }' "${file_path}" 2>/dev/null || true)"
    if [ -z "${value}" ]; then
        printf '0'
    else
        printf '%s' "${value}"
    fi
}

run_aether_fsck() {
    local workspace="$1"
    local output_prefix="$2"

    measure_command "fsck" "${output_prefix}" "${AETHER_BIN}" --workspace "${workspace}" fsck

    FSCK_MISSING_VECTORS="$(_extract_metric_count 'Symbols missing vectors' "${MEASURE_STDOUT_FILE}")"
    FSCK_MISSING_GRAPH_NODES="$(_extract_metric_count 'Symbols missing graph nodes' "${MEASURE_STDOUT_FILE}")"
    FSCK_PHANTOM_GRAPH_NODES="$(_extract_metric_count 'Phantom graph nodes' "${MEASURE_STDOUT_FILE}")"
    FSCK_DANGLING_EDGES="$(_extract_metric_count 'Dangling edges' "${MEASURE_STDOUT_FILE}")"
    FSCK_ORPHANED_VECTORS="$(_extract_metric_count 'Orphaned vectors' "${MEASURE_STDOUT_FILE}")"
    FSCK_INCOMPLETE_INTENTS="$(_extract_metric_count 'Incomplete write intents' "${MEASURE_STDOUT_FILE}")"

    FSCK_ISSUE_COUNT=$((
        FSCK_MISSING_VECTORS +
        FSCK_MISSING_GRAPH_NODES +
        FSCK_PHANTOM_GRAPH_NODES +
        FSCK_DANGLING_EDGES +
        FSCK_ORPHANED_VECTORS +
        FSCK_INCOMPLETE_INTENTS
    ))

    if [ "${MEASURE_EXIT_CODE}" -ne 0 ]; then
        FSCK_STATUS="fail"
    elif [ "${FSCK_ISSUE_COUNT}" -eq 0 ]; then
        FSCK_STATUS="ok"
    else
        FSCK_STATUS="issues"
    fi

    return 0
}

run_aether_status() {
    local workspace="$1"
    local output_prefix="$2"

    measure_command "status" "${output_prefix}" "${AETHER_BIN}" --workspace "${workspace}" status

    local coverage_line
    coverage_line="$(grep -m1 '^SIR Coverage:' "${MEASURE_STDOUT_FILE}" || true)"
    if [ -z "${coverage_line}" ]; then
        STATUS_SYMBOLS_WITH_SIR=0
        STATUS_TOTAL_SYMBOLS=0
        STATUS_SIR_PERCENTAGE="0.0"
        return 0
    fi

    STATUS_SYMBOLS_WITH_SIR="$(printf '%s\n' "${coverage_line}" | sed -n 's/^SIR Coverage: \([0-9][0-9]*\) \/ \([0-9][0-9]*\) (\([0-9.][0-9.]*\)%).*/\1/p')"
    STATUS_TOTAL_SYMBOLS="$(printf '%s\n' "${coverage_line}" | sed -n 's/^SIR Coverage: \([0-9][0-9]*\) \/ \([0-9][0-9]*\) (\([0-9.][0-9.]*\)%).*/\2/p')"
    STATUS_SIR_PERCENTAGE="$(printf '%s\n' "${coverage_line}" | sed -n 's/^SIR Coverage: \([0-9][0-9]*\) \/ \([0-9][0-9]*\) (\([0-9.][0-9.]*\)%).*/\3/p')"

    STATUS_SYMBOLS_WITH_SIR="${STATUS_SYMBOLS_WITH_SIR:-0}"
    STATUS_TOTAL_SYMBOLS="${STATUS_TOTAL_SYMBOLS:-0}"
    STATUS_SIR_PERCENTAGE="${STATUS_SIR_PERCENTAGE:-0.0}"

    return 0
}

run_aether_health() {
    local workspace="$1"
    local output_prefix="$2"

    measure_command "health" "${output_prefix}" "${AETHER_BIN}" --workspace "${workspace}" health

    HEALTH_TOTAL_EDGES="$(grep -m1 '"total_edges"' "${MEASURE_STDOUT_FILE}" | tr -cd '0-9' || true)"
    HEALTH_TOTAL_EDGES="${HEALTH_TOTAL_EDGES:-0}"

    return 0
}

check_ollama() {
    curl -sSf --max-time 5 "http://127.0.0.1:11434/api/tags" >/dev/null

    # Pre-warm the model to avoid cold-start timeouts
    local model_name
    model_name="${OLLAMA_MODEL:-qwen3.5:9b}"
    log_info "Pre-warming Ollama model: ${model_name}"
    curl -s http://127.0.0.1:11434/api/generate \
        -d "{\"model\":\"${model_name}\",\"prompt\":\"hi\",\"stream\":false}" > /dev/null 2>&1
}

check_api_key() {
    local env_var_name="$1"
    local value="${!env_var_name:-}"
    [ -n "${value}" ]
}

check_bench_dir() {
    local bench_dir="$1"

    if [ -e "${bench_dir}" ] && [ ! -d "${bench_dir}" ]; then
        log_error "Bench path exists but is not a directory: ${bench_dir}"
        return 1
    fi

    mkdir -p "${bench_dir}" || {
        log_error "Failed to create benchmark directory: ${bench_dir}"
        return 1
    }

    local backend
    backend="$(detect_storage_backend "${bench_dir}")"
    if [ "${backend}" = "tmpfs" ]; then
        log_warn "Benchmark directory is on tmpfs (${bench_dir}); this can cause RAM pressure and OOMs"
    fi

    if [[ "${bench_dir}" == /tmp* ]]; then
        log_warn "Bench dir under /tmp may be RAM-backed on WSL2; consider --bench-dir /mnt/d/aether-bench"
    fi

    return 0
}

write_provider_config() {
    local workspace="$1"
    local provider="$2"
    local config_dir="${workspace}/.aether"
    local config_path="${config_dir}/config.toml"

    if [ "${provider}" = "pass1-only" ]; then
        return 0
    fi

    mkdir -p "${config_dir}"

    case "${provider}" in
        ollama)
            cat > "${config_path}" <<'CFG'
[inference]
provider = "qwen3_local"
model = "qwen3.5:9b"
endpoint = "http://127.0.0.1:11434"
CFG
            ;;
        gemini)
            cat > "${config_path}" <<'CFG'
[inference]
provider = "gemini"
model = "gemini-flash-latest"
api_key_env = "GEMINI_API_KEY"
CFG
            ;;
        nim)
            cat > "${config_path}" <<'CFG'
[inference]
provider = "openai_compat"
model = "qwen3.5-397b-a17b"
endpoint = "https://integrate.api.nvidia.com/v1"
api_key_env = "NVIDIA_NIM_API_KEY"
CFG
            ;;
        tiered)
            cat > "${config_path}" <<'CFG'
[inference]
provider = "tiered"

[inference.tiered]
primary = "openai_compat"
primary_model = "qwen3.5-397b-a17b"
primary_endpoint = "https://integrate.api.nvidia.com/v1"
primary_api_key_env = "NVIDIA_NIM_API_KEY"
primary_threshold = 0.8
fallback_model = "qwen3.5:9b"
fallback_endpoint = "http://127.0.0.1:11434"
retry_with_fallback = true
CFG
            ;;
        *)
            log_error "Unknown provider for config generation: ${provider}"
            return 1
            ;;
    esac

    return 0
}

detect_storage_backend() {
    local path="$1"
    local fs_type

    fs_type="$(df -T "${path}" 2>/dev/null | awk 'NR==2 { print $2 }')"

    if [ "${fs_type}" = "tmpfs" ]; then
        printf 'tmpfs'
        return 0
    fi

    if [ "${fs_type}" = "9p" ] || [[ "${path}" =~ ^/mnt/[a-zA-Z]/ ]]; then
        printf 'windows-9p'
        return 0
    fi

    if [ "${fs_type}" = "ext4" ]; then
        printf 'native-ext4'
        return 0
    fi

    if [ -n "${fs_type}" ]; then
        printf '%s' "${fs_type}"
    else
        printf 'unknown'
    fi
}

run_query_benchmark() {
    local workspace="$1"
    local query_type="$2"
    local output_prefix="$3"
    shift 3

    mkdir -p "$(dirname "${output_prefix}")"
    QUERY_BENCH_RESPONSE_FILE="${output_prefix}.response.log"

    case "${query_type}" in
        lexical)
            local query="$1"
            measure_command "query_lexical" "${output_prefix}" \
                "${AETHER_BIN}" --workspace "${workspace}" --search "${query}" --search-mode lexical --output json --search-limit 20
            QUERY_BENCH_LATENCY_MS="${MEASURE_WALL_MS}"
            QUERY_BENCH_EXIT_CODE="${MEASURE_EXIT_CODE}"
            QUERY_BENCH_RESPONSE_FILE="${MEASURE_STDOUT_FILE}"
            ;;
        call_chain)
            local base_url="$1"
            local symbol_id="$2"
            local escaped_symbol_id
            escaped_symbol_id="$(json_escape "${symbol_id}")"
            local payload
            payload="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"aether_call_chain\",\"arguments\":{\"symbol_id\":\"${escaped_symbol_id}\",\"max_depth\":5}}}"

            local start_ns
            local end_ns
            start_ns="$(date +%s%N)"
            set +e
            curl -sS --max-time 120 \
                -H 'content-type: application/json' \
                -H 'accept: application/json, text/event-stream' \
                --data "${payload}" \
                "${base_url}/mcp" > "${QUERY_BENCH_RESPONSE_FILE}"
            QUERY_BENCH_EXIT_CODE=$?
            set -e
            end_ns="$(date +%s%N)"
            QUERY_BENCH_LATENCY_MS="$(awk -v start="${start_ns}" -v end="${end_ns}" 'BEGIN { printf "%.3f", (end - start) / 1000000 }')"
            ;;
        get_sir)
            local base_url="$1"
            local symbol_id="$2"
            local escaped_symbol_id
            escaped_symbol_id="$(json_escape "${symbol_id}")"
            local payload
            payload="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"aether_get_sir\",\"arguments\":{\"level\":\"leaf\",\"symbol_id\":\"${escaped_symbol_id}\"}}}"

            local start_ns
            local end_ns
            start_ns="$(date +%s%N)"
            set +e
            curl -sS --max-time 120 \
                -H 'content-type: application/json' \
                -H 'accept: application/json, text/event-stream' \
                --data "${payload}" \
                "${base_url}/mcp" > "${QUERY_BENCH_RESPONSE_FILE}"
            QUERY_BENCH_EXIT_CODE=$?
            set -e
            end_ns="$(date +%s%N)"
            QUERY_BENCH_LATENCY_MS="$(awk -v start="${start_ns}" -v end="${end_ns}" 'BEGIN { printf "%.3f", (end - start) / 1000000 }')"
            ;;
        *)
            log_error "Unknown query benchmark type: ${query_type}"
            return 1
            ;;
    esac

    return 0
}

kill_and_recover() {
    local pid="$1"
    local workspace="$2"
    local output_prefix="$3"

    set +e
    kill -9 "${pid}" 2>/dev/null
    wait "${pid}" 2>/dev/null
    set -e

    measure_command "crash_recovery" "${output_prefix}" "${AETHER_BIN}" --workspace "${workspace}" --index-once --full

    local replayed
    replayed="$(
        {
            grep -Eo 'Replayed [0-9]+ incomplete write intents' "${MEASURE_STDERR_FILE}" 2>/dev/null
            grep -Eo 'Replayed [0-9]+ incomplete write intents' "${MEASURE_STDOUT_FILE}" 2>/dev/null
        } | head -n1 | grep -Eo '[0-9]+' || true
    )"

    RECOVERY_WALL_MS="${MEASURE_WALL_MS}"
    RECOVERY_EXIT_CODE="${MEASURE_EXIT_CODE}"
    RECOVERY_REPLAYED_INTENTS="${replayed:-0}"

    return 0
}

extract_symbol_ids_from_search_json() {
    local search_output_file="$1"
    local limit="$2"

    if [ ! -f "${search_output_file}" ]; then
        return 0
    fi

    grep -o '"symbol_id"[[:space:]]*:[[:space:]]*"[^"]*"' "${search_output_file}" \
        | sed -E 's/.*"symbol_id"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/' \
        | awk '!seen[$0]++' \
        | head -n "${limit}"
}

extract_symbol_ids_from_health_json() {
    local health_output_file="$1"
    local limit="$2"

    if [ ! -f "${health_output_file}" ]; then
        return 0
    fi

    grep -o '"symbol_id"[[:space:]]*:[[:space:]]*"[^"]*"' "${health_output_file}" \
        | sed -E 's/.*"symbol_id"[[:space:]]*:[[:space:]]*"([^"]*)".*/\1/' \
        | awk '!seen[$0]++' \
        | head -n "${limit}"
}
