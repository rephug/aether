#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
REPOS_FILE="${SCRIPT_DIR}/repos.toml"

# shellcheck source=tests/stress/benchmark_helpers.sh
source "${SCRIPT_DIR}/benchmark_helpers.sh"

TIER="small"
PROVIDER="ollama"
BENCH_DIR="/mnt/d/aether-bench"
REPORT_DIR="tests/stress/reports"
SKIP_CLONE=0
ENABLE_CONCURRENT="${AETHER_BENCH_ENABLE_CONCURRENT:-0}"

CHILD_PIDS=()

PASS2_SAMPLE_COUNT=0
PASS2_SUCCESS_COUNT=0
PASS2_FAILURE_COUNT=0
PASS2_SKIPPED_COUNT=0
PASS2_P50_S="n/a"
PASS2_P95_S="n/a"
PASS2_P99_S="n/a"
PASS2_QUEUE_RATE="n/a"
PASS2_PROVIDER_SPLIT="n/a"
PASS2_EXIT_STATUS="not-run"
PASS2_OUTCOME_FILE=""
PASS2_WALL_S="n/a"

usage() {
    cat <<'USAGE'
Usage: ./tests/stress/run_benchmark.sh [options]

Options:
  --tier small|medium|large|all        Benchmark tier to run (default: small)
  --provider ollama|pass1-only|gemini|nim|tiered
                                       Inference provider mode (default: ollama)
  --bench-dir <path>                   Benchmark workspace root (default: /mnt/d/aether-bench)
  --report-dir <path>                  Report output directory (default: tests/stress/reports)
  --skip-clone                         Reuse existing clones in bench dir
  --help                               Show this help message
USAGE
}

cleanup_background() {
    local pid
    for pid in "${CHILD_PIDS[@]:-}"; do
        if kill -0 "${pid}" 2>/dev/null; then
            kill "${pid}" 2>/dev/null || true
            wait "${pid}" 2>/dev/null || true
        fi
    done
}

trap cleanup_background EXIT INT TERM

resolve_path() {
    local input_path="$1"
    if [[ "${input_path}" = /* ]]; then
        printf '%s' "${input_path}"
    else
        printf '%s' "${REPO_ROOT}/${input_path}"
    fi
}

format_seconds_from_ms() {
    local ms="$1"
    awk -v value="${ms}" 'BEGIN { printf "%.3fs", value / 1000 }'
}

format_megabytes_from_kb() {
    local kb="$1"
    awk -v value="${kb}" 'BEGIN { printf "%.1fMB", value / 1024 }'
}

extract_json_field() {
    local line="$1"
    local key="$2"
    printf '%s\n' "${line}" | sed -n "s/.*\"${key}\":\"\([^\"]*\)\".*/\1/p"
}

provider_description() {
    case "${PROVIDER}" in
        ollama)
            printf 'ollama (qwen3_local: qwen3.5:9b)'
            ;;
        pass1-only)
            printf 'pass1-only (no inference)'
            ;;
        gemini)
            printf 'gemini (gemini-flash-latest)'
            ;;
        nim)
            printf 'openai_compat (NVIDIA NIM qwen3.5-397b-a17b)'
            ;;
        tiered)
            printf 'tiered (openai_compat primary -> qwen3_local fallback, threshold 0.8)'
            ;;
    esac
}

write_provider_config_block() {
    case "${PROVIDER}" in
        ollama)
            cat <<'CFG'
[inference]
provider = "qwen3_local"
model = "qwen3.5:9b"
endpoint = "http://127.0.0.1:11434"
CFG
            ;;
        pass1-only)
            cat <<'CFG'
# pass1-only mode does not require inference config
CFG
            ;;
        gemini)
            cat <<'CFG'
[inference]
provider = "gemini"
model = "gemini-flash-latest"
api_key_env = "GEMINI_API_KEY"
CFG
            ;;
        nim)
            cat <<'CFG'
[inference]
provider = "openai_compat"
model = "qwen3.5-397b-a17b"
endpoint = "https://integrate.api.nvidia.com/v1"
api_key_env = "NVIDIA_NIM_API_KEY"
CFG
            ;;
        tiered)
            cat <<'CFG'
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
    esac
}

parse_repos_for_tier() {
    local selected_tier="$1"
    awk -v selected_tier="${selected_tier}" '
        function include_tier(repo_tier, wanted) {
            if (wanted == "all") {
                return 1
            }
            if (wanted == "small") {
                return repo_tier == "small"
            }
            if (wanted == "medium") {
                return repo_tier == "small" || repo_tier == "medium"
            }
            if (wanted == "large") {
                return repo_tier == "large"
            }
            return 0
        }

        function reset_repo() {
            name = ""
            url = ""
            commit = ""
            tier = ""
            language = ""
            approx_symbols = ""
            clone_size_mb = ""
        }

        function flush_repo() {
            if (name == "" || url == "" || tier == "") {
                return
            }
            if (!include_tier(tier, selected_tier)) {
                return
            }
            print name "\t" url "\t" commit "\t" tier "\t" language "\t" approx_symbols "\t" clone_size_mb
        }

        BEGIN {
            in_repo = 0
            reset_repo()
        }

        /^\[\[repo\]\]/ {
            if (in_repo) {
                flush_repo()
            }
            reset_repo()
            in_repo = 1
            next
        }

        !in_repo { next }
        /^[[:space:]]*#/ { next }
        /^[[:space:]]*$/ { next }

        {
            split($0, parts, "=")
            key = parts[1]
            value = substr($0, index($0, "=") + 1)

            gsub(/^[ \t]+|[ \t]+$/, "", key)
            gsub(/^[ \t]+|[ \t]+$/, "", value)
            gsub(/^"|"$/, "", value)

            if (key == "name") name = value
            else if (key == "url") url = value
            else if (key == "commit") commit = value
            else if (key == "tier") tier = value
            else if (key == "language") language = value
            else if (key == "approx_symbols") approx_symbols = value
            else if (key == "clone_size_mb") clone_size_mb = value
        }

        END {
            if (in_repo) {
                flush_repo()
            }
        }
    ' "${REPOS_FILE}"
}

validate_provider_requirements() {
    case "${PROVIDER}" in
        ollama)
            if ! check_ollama; then
                log_error "Ollama is not reachable at http://127.0.0.1:11434"
                log_error "Start Ollama and pull qwen3.5:9b, or run --provider pass1-only"
                exit 1
            fi
            ;;
        pass1-only)
            ;;
        gemini)
            if ! check_api_key "GEMINI_API_KEY"; then
                log_error "Missing GEMINI_API_KEY for --provider gemini"
                exit 1
            fi
            ;;
        nim)
            if ! check_api_key "NVIDIA_NIM_API_KEY"; then
                log_error "Missing NVIDIA_NIM_API_KEY for --provider nim"
                exit 1
            fi
            ;;
        tiered)
            if ! check_api_key "NVIDIA_NIM_API_KEY"; then
                log_error "Missing NVIDIA_NIM_API_KEY for --provider tiered"
                log_error "Use --provider ollama if cloud key is unavailable"
                exit 1
            fi
            if ! check_ollama; then
                log_error "Ollama is required for --provider tiered fallback"
                exit 1
            fi
            ;;
        *)
            log_error "Unsupported provider: ${PROVIDER}"
            exit 1
            ;;
    esac
}

start_query_server() {
    local workspace="$1"
    local log_prefix="$2"

    build_aether_query

    local port
    port="$((9700 + (RANDOM % 800)))"
    local bind_addr="127.0.0.1:${port}"
    local stdout_file="${log_prefix}.query.stdout.log"
    local stderr_file="${log_prefix}.query.stderr.log"

    "${AETHER_QUERY_BIN}" serve --index-path "${workspace}" --bind "${bind_addr}" >"${stdout_file}" 2>"${stderr_file}" &
    local query_pid=$!
    CHILD_PIDS+=("${query_pid}")

    local ready=0
    local attempt
    for attempt in $(seq 1 30); do
        if curl -fsS --max-time 2 "http://${bind_addr}/health" >/dev/null 2>&1; then
            ready=1
            break
        fi
        sleep 1
    done

    if [ "${ready}" -ne 1 ]; then
        log_warn "aether-query failed to become ready at ${bind_addr}"
        kill "${query_pid}" 2>/dev/null || true
        wait "${query_pid}" 2>/dev/null || true
        printf '\t\t\n'
        return 1
    fi

    printf '%s\t%s\n' "${query_pid}" "http://${bind_addr}"
    return 0
}

stop_query_server() {
    local query_pid="$1"
    if kill -0 "${query_pid}" 2>/dev/null; then
        kill "${query_pid}" 2>/dev/null || true
        wait "${query_pid}" 2>/dev/null || true
    fi
}

run_pass2_sample() {
    local workspace="$1"
    local log_dir="$2"
    local sample_target="${3:-100}"

    local stdout_file="${log_dir}/phase_b_pass2.stdout.log"
    local stderr_file="${log_dir}/phase_b_pass2.stderr.log"
    local time_file="${log_dir}/phase_b_pass2.time.log"
    local timestamp_file="${log_dir}/phase_b_pass2.timestamps.ns"
    local interval_file="${log_dir}/phase_b_pass2.intervals.s"
    local outcome_file="${log_dir}/phase_b_pass2.outcomes.log"

    mkdir -p "${log_dir}"
    : > "${stdout_file}"
    : > "${stderr_file}"
    : > "${timestamp_file}"

    local start_ns
    local end_ns
    start_ns="$(date +%s%N)"

    set +e
    /usr/bin/time -v -o "${time_file}" "${AETHER_BIN}" --workspace "${workspace}" --index-once --full --print-sir >"${stdout_file}" 2>"${stderr_file}" &
    local index_pid=$!
    set -e

    local reached_sample=0
    local observed=0
    local current_count

    while kill -0 "${index_pid}" 2>/dev/null; do
        current_count="$(grep -Ec '^SIR_(STORED|STALE|SKIPPED)' "${stdout_file}" 2>/dev/null || true)"
        if [ "${current_count}" -gt "${observed}" ]; then
            local now_ns
            now_ns="$(date +%s%N)"
            while [ "${observed}" -lt "${current_count}" ]; do
                printf '%s\n' "${now_ns}" >> "${timestamp_file}"
                observed=$((observed + 1))
            done
        fi

        if [ "${current_count}" -ge "${sample_target}" ]; then
            reached_sample=1
            kill -TERM "${index_pid}" 2>/dev/null || true
            sleep 2
            if kill -0 "${index_pid}" 2>/dev/null; then
                kill -KILL "${index_pid}" 2>/dev/null || true
            fi
            break
        fi

        sleep 1
    done

    set +e
    wait "${index_pid}"
    local exit_code=$?
    set -e

    end_ns="$(date +%s%N)"

    current_count="$(grep -Ec '^SIR_(STORED|STALE|SKIPPED)' "${stdout_file}" 2>/dev/null || true)"
    if [ "${current_count}" -gt "${observed}" ]; then
        local now_ns
        now_ns="$(date +%s%N)"
        while [ "${observed}" -lt "${current_count}" ]; do
            printf '%s\n' "${now_ns}" >> "${timestamp_file}"
            observed=$((observed + 1))
        done
    fi

    grep -E '^SIR_(STORED|STALE|SKIPPED)' "${stdout_file}" | head -n "${sample_target}" > "${outcome_file}" || true

    PASS2_OUTCOME_FILE="${outcome_file}"
    PASS2_SAMPLE_COUNT="$(wc -l < "${outcome_file}" | tr -d '[:space:]')"
    PASS2_SUCCESS_COUNT="$(grep -c '^SIR_STORED' "${outcome_file}" || true)"
    PASS2_FAILURE_COUNT="$(grep -c '^SIR_STALE' "${outcome_file}" || true)"
    PASS2_SKIPPED_COUNT="$(grep -c '^SIR_SKIPPED' "${outcome_file}" || true)"

    PASS2_WALL_S="$(awk -v start="${start_ns}" -v end="${end_ns}" 'BEGIN { printf "%.3f", (end - start) / 1000000000 }')"
    local wall_ms
    wall_ms="$(awk -v start="${start_ns}" -v end="${end_ns}" 'BEGIN { printf "%.3f", (end - start) / 1000000 }')"

    if [ "${reached_sample}" -eq 1 ]; then
        PASS2_EXIT_STATUS="sampled-stop"
    else
        PASS2_EXIT_STATUS="exit-${exit_code}"
    fi

    awk 'NR == 1 { prev = $1; next } { printf "%.6f\n", ($1 - prev) / 1000000000; prev = $1 }' "${timestamp_file}" > "${interval_file}" || true
    PASS2_P50_S="$(percentile_from_file "${interval_file}" 50)"
    PASS2_P95_S="$(percentile_from_file "${interval_file}" 95)"
    PASS2_P99_S="$(percentile_from_file "${interval_file}" 99)"

    if [ "${PASS2_SAMPLE_COUNT}" -gt 0 ]; then
        PASS2_QUEUE_RATE="$(awk -v count="${PASS2_SAMPLE_COUNT}" -v seconds="${PASS2_WALL_S}" 'BEGIN { if (seconds <= 0) { printf "n/a" } else { printf "%.2f", count / seconds } }')"
    else
        PASS2_QUEUE_RATE="n/a"
    fi

    if [ "${PROVIDER}" = "tiered" ]; then
        PASS2_PROVIDER_SPLIT="unavailable (tiered routing not emitted per symbol)"
    else
        local provider_counts
        provider_counts="$({
            awk '
                /^SIR_STORED/ {
                    for (i = 1; i <= NF; i++) {
                        if ($i ~ /^provider=/) {
                            split($i, kv, "=")
                            counts[kv[2]] += 1
                        }
                    }
                }
                END {
                    first = 1
                    for (provider in counts) {
                        if (!first) {
                            printf ", "
                        }
                        printf "%s=%d", provider, counts[provider]
                        first = 0
                    }
                }
            ' "${outcome_file}"
        } || true)"
        if [ -z "${provider_counts}" ]; then
            PASS2_PROVIDER_SPLIT="n/a"
        else
            PASS2_PROVIDER_SPLIT="${provider_counts}"
        fi
    fi

    # Preserve time-derived peak RSS for optional diagnostics.
    MEASURE_TIME_FILE="${time_file}"
    MEASURE_PEAK_RSS_KB="$((
        $(awk -F: '/Maximum resident set size \(kbytes\)/ { gsub(/^[ \t]+/, "", $2); print $2; exit }' "${time_file}" 2>/dev/null || echo 0)
    ))"

    return 0
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --tier)
            TIER="${2:-}"
            shift 2
            ;;
        --provider)
            PROVIDER="${2:-}"
            shift 2
            ;;
        --bench-dir)
            BENCH_DIR="${2:-}"
            shift 2
            ;;
        --report-dir)
            REPORT_DIR="${2:-}"
            shift 2
            ;;
        --skip-clone)
            SKIP_CLONE=1
            shift
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            log_error "Unknown argument: $1"
            usage
            exit 1
            ;;
    esac
done

case "${TIER}" in
    small|medium|large|all)
        ;;
    *)
        log_error "Invalid --tier value: ${TIER}"
        exit 1
        ;;
esac

case "${PROVIDER}" in
    ollama|pass1-only|gemini|nim|tiered)
        ;;
    *)
        log_error "Invalid --provider value: ${PROVIDER}"
        exit 1
        ;;
esac

BENCH_DIR="$(resolve_path "${BENCH_DIR}")"
REPORT_DIR="$(resolve_path "${REPORT_DIR}")"

if [ ! -f "${REPOS_FILE}" ]; then
    log_error "Missing repos definition file: ${REPOS_FILE}"
    exit 1
fi

mkdir -p "${REPORT_DIR}"
check_bench_dir "${BENCH_DIR}"
validate_provider_requirements

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_TMP_DIR="${BENCH_DIR}/.aether-bench-tmp/${RUN_ID}"
mkdir -p "${RUN_TMP_DIR}"

JSONL_RESULTS="${RUN_TMP_DIR}/repo_results.jsonl"
: > "${JSONL_RESULTS}"

REPORT_JSON="${REPORT_DIR}/aether_scale_report_${RUN_ID}.json"
REPORT_MD="${REPORT_DIR}/aether_scale_report_${RUN_ID}.md"

STORAGE_BACKEND="$(detect_storage_backend "${BENCH_DIR}")"
AETHER_COMMIT="$(git -C "${REPO_ROOT}" rev-parse --short HEAD)"
AETHER_VERSION="$(git -C "${REPO_ROOT}" describe --tags --always --dirty 2>/dev/null || echo "${AETHER_COMMIT}")"
SYSTEM_INFO="$(uname -srmo 2>/dev/null || uname -a)"

log_info "Run ID: ${RUN_ID}"
log_info "Tier: ${TIER}"
log_info "Provider: ${PROVIDER}"
log_info "Bench dir: ${BENCH_DIR}"
log_info "Report dir: ${REPORT_DIR}"

build_aether

mapfile -t REPO_ROWS < <(parse_repos_for_tier "${TIER}")
if [ "${#REPO_ROWS[@]}" -eq 0 ]; then
    log_error "No repositories selected for tier: ${TIER}"
    exit 1
fi

for row in "${REPO_ROWS[@]}"; do
    IFS=$'\t' read -r repo_name repo_url repo_commit repo_tier repo_language repo_approx_symbols repo_clone_size <<< "${row}"

    log_info "----- Running benchmark for ${repo_name} (${repo_tier}) -----"

    repo_dir="${BENCH_DIR}/${repo_name}"
    repo_log_dir="${RUN_TMP_DIR}/${repo_name}"
    mkdir -p "${repo_log_dir}"

    resolved_sha=""
    repo_status="ok"
    error_message=""

    pass1_time="n/a"
    pass1_peak_memory="n/a"
    pass1_symbol_count="0"
    pass1_edge_count="0"
    pass1_fsck_status="not-run"

    pass2_success_rate="n/a"
    pass2_provider_split="n/a"
    pass2_p50="n/a"
    pass2_p95="n/a"
    pass2_p99="n/a"
    pass2_sample_count="0"
    pass2_fsck_status="not-run"

    lexical_p50="n/a"
    lexical_p95="n/a"
    lexical_p99="n/a"
    call_chain_p50="n/a"
    call_chain_p95="n/a"
    call_chain_p99="n/a"
    get_sir_cached_p50="n/a"
    get_sir_cached_p95="n/a"
    get_sir_cached_p99="n/a"
    get_sir_ondemand_p50="n/a"
    get_sir_ondemand_p95="n/a"
    get_sir_ondemand_p99="n/a"

    crash_recovery_time="skipped"
    crash_replayed_intents="0"
    crash_fsck_status="not-run"

    PASS2_SAMPLE_COUNT=0
    PASS2_SUCCESS_COUNT=0
    PASS2_FAILURE_COUNT=0
    PASS2_SKIPPED_COUNT=0
    PASS2_P50_S="n/a"
    PASS2_P95_S="n/a"
    PASS2_P99_S="n/a"
    PASS2_QUEUE_RATE="n/a"
    PASS2_PROVIDER_SPLIT="n/a"
    PASS2_EXIT_STATUS="not-run"
    PASS2_OUTCOME_FILE=""
    PASS2_WALL_S="n/a"

    if [ "${SKIP_CLONE}" -eq 1 ]; then
        if [ ! -d "${repo_dir}/.git" ]; then
            repo_status="error"
            error_message="--skip-clone requested but clone missing: ${repo_dir}"
        else
            resolved_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
        fi
    else
        if ! clone_repo "${repo_name}" "${repo_url}" "${repo_commit}" "${repo_dir}"; then
            repo_status="error"
            error_message="clone failed"
        else
            resolved_sha="${CLONE_RESOLVED_SHA}"
        fi
    fi

    if [ "${repo_status}" = "ok" ]; then
        rm -rf "${repo_dir}/.aether"
        if ! write_provider_config "${repo_dir}" "${PROVIDER}"; then
            repo_status="error"
            error_message="failed to write provider config"
        fi
    fi

    phase_a_health_output=""

    if [ "${repo_status}" = "ok" ]; then
        measure_command "phase_a_pass1" "${repo_log_dir}/phase_a_pass1" "${AETHER_BIN}" --workspace "${repo_dir}" --index-once
        pass1_time="$(format_seconds_from_ms "${MEASURE_WALL_MS}")"
        pass1_peak_memory="$(format_megabytes_from_kb "${MEASURE_PEAK_RSS_KB}")"

        if [ "${MEASURE_EXIT_CODE}" -ne 0 ]; then
            repo_status="error"
            error_message="pass1 indexing failed"
        fi
    fi

    if [ "${repo_status}" = "ok" ]; then
        run_aether_status "${repo_dir}" "${repo_log_dir}/phase_a_status"
        pass1_symbol_count="${STATUS_TOTAL_SYMBOLS}"

        run_aether_health "${repo_dir}" "${repo_log_dir}/phase_a_health"
        pass1_edge_count="${HEALTH_TOTAL_EDGES}"
        phase_a_health_output="${MEASURE_STDOUT_FILE}"

        run_aether_fsck "${repo_dir}" "${repo_log_dir}/phase_a_fsck"
        pass1_fsck_status="${FSCK_STATUS}"
        if [ "${FSCK_STATUS}" = "fail" ]; then
            repo_status="error"
            error_message="fsck failed after pass1"
        fi
    fi

    if [ "${repo_status}" = "ok" ] && [ "${PROVIDER}" != "pass1-only" ]; then
        run_pass2_sample "${repo_dir}" "${repo_log_dir}"
        pass2_sample_count="${PASS2_SAMPLE_COUNT}"
        pass2_provider_split="${PASS2_PROVIDER_SPLIT}"
        pass2_p50="${PASS2_P50_S}"
        pass2_p95="${PASS2_P95_S}"
        pass2_p99="${PASS2_P99_S}"

        if [ "${PASS2_SAMPLE_COUNT}" -gt 0 ]; then
            pass2_success_rate="$(awk -v success="${PASS2_SUCCESS_COUNT}" -v total="${PASS2_SAMPLE_COUNT}" 'BEGIN { printf "%.1f%% (%d/%d)", (success * 100.0) / total, success, total }')"
        else
            pass2_success_rate="n/a"
        fi

        run_aether_fsck "${repo_dir}" "${repo_log_dir}/phase_b_fsck"
        pass2_fsck_status="${FSCK_STATUS}"
    fi

    if [ "${repo_status}" = "ok" ]; then
        query_seed_json="${repo_log_dir}/phase_c_seed_search.json"
        set +e
        "${AETHER_BIN}" --workspace "${repo_dir}" --search "a" --search-mode lexical --output json --search-limit 200 >"${query_seed_json}" 2>"${repo_log_dir}/phase_c_seed_search.stderr.log"
        set -e

        mapfile -t search_symbol_ids < <(extract_symbol_ids_from_search_json "${query_seed_json}" 50)

        lexical_samples_file="${repo_log_dir}/phase_c_lexical.latency.ms"
        : > "${lexical_samples_file}"
        lexical_queries=("main" "init" "config" "error" "test" "build" "service" "handler" "model" "parse")
        for lexical_query in "${lexical_queries[@]}"; do
            run_query_benchmark "${repo_dir}" "lexical" "${repo_log_dir}/phase_c_lexical_${lexical_query}" "${lexical_query}"
            if [ "${QUERY_BENCH_EXIT_CODE}" -eq 0 ]; then
                printf '%s\n' "${QUERY_BENCH_LATENCY_MS}" >> "${lexical_samples_file}"
            fi
        done

        lexical_p50="$(percentile_from_file "${lexical_samples_file}" 50)"
        lexical_p95="$(percentile_from_file "${lexical_samples_file}" 95)"
        lexical_p99="$(percentile_from_file "${lexical_samples_file}" 99)"

        query_server_info=""
        if query_server_info="$(start_query_server "${repo_dir}" "${repo_log_dir}/phase_c")"; then
            query_pid="$(printf '%s\n' "${query_server_info}" | awk -F'\t' 'NR==1 { print $1 }')"
            query_base_url="$(printf '%s\n' "${query_server_info}" | awk -F'\t' 'NR==1 { print $2 }')"

            call_chain_samples_file="${repo_log_dir}/phase_c_call_chain.latency.ms"
            : > "${call_chain_samples_file}"

            if [ -n "${phase_a_health_output}" ]; then
                mapfile -t health_symbol_ids < <(extract_symbol_ids_from_health_json "${phase_a_health_output}" 20)
            else
                health_symbol_ids=()
            fi

            if [ "${#health_symbol_ids[@]}" -eq 0 ] && [ "${#search_symbol_ids[@]}" -gt 0 ]; then
                health_symbol_ids=("${search_symbol_ids[@]}")
            fi

            if [ "${#health_symbol_ids[@]}" -gt 0 ]; then
                for idx in 0 1 2 3 4; do
                    selected_id="${health_symbol_ids[$((idx % ${#health_symbol_ids[@]}))]}"
                    run_query_benchmark "${repo_dir}" "call_chain" "${repo_log_dir}/phase_c_call_chain_${idx}" "${query_base_url}" "${selected_id}"
                    if [ "${QUERY_BENCH_EXIT_CODE}" -eq 0 ]; then
                        printf '%s\n' "${QUERY_BENCH_LATENCY_MS}" >> "${call_chain_samples_file}"
                    fi
                done
            fi

            call_chain_p50="$(percentile_from_file "${call_chain_samples_file}" 50)"
            call_chain_p95="$(percentile_from_file "${call_chain_samples_file}" 95)"
            call_chain_p99="$(percentile_from_file "${call_chain_samples_file}" 99)"

            cached_samples_file="${repo_log_dir}/phase_c_get_sir_cached.latency.ms"
            ondemand_samples_file="${repo_log_dir}/phase_c_get_sir_ondemand.latency.ms"
            : > "${cached_samples_file}"
            : > "${ondemand_samples_file}"

            cached_ids=()
            if [ -n "${PASS2_OUTCOME_FILE}" ] && [ -f "${PASS2_OUTCOME_FILE}" ]; then
                mapfile -t cached_ids < <(grep '^SIR_STORED' "${PASS2_OUTCOME_FILE}" | sed -E 's/^SIR_STORED symbol_id=([^ ]+).*/\1/' | awk '!seen[$0]++' | head -n 5)
            fi

            if [ "${#cached_ids[@]}" -gt 0 ]; then
                for cached_id in "${cached_ids[@]}"; do
                    run_query_benchmark "${repo_dir}" "get_sir" "${repo_log_dir}/phase_c_get_sir_cached_${cached_id}" "${query_base_url}" "${cached_id}"
                    if [ "${QUERY_BENCH_EXIT_CODE}" -eq 0 ]; then
                        printf '%s\n' "${QUERY_BENCH_LATENCY_MS}" >> "${cached_samples_file}"
                    fi
                done
            fi

            ondemand_ids=()
            if [ "${#search_symbol_ids[@]}" -gt 0 ]; then
                for search_id in "${search_symbol_ids[@]}"; do
                    skip=0
                    for cached_id in "${cached_ids[@]}"; do
                        if [ "${search_id}" = "${cached_id}" ]; then
                            skip=1
                            break
                        fi
                    done
                    if [ "${skip}" -eq 0 ]; then
                        ondemand_ids+=("${search_id}")
                    fi
                    if [ "${#ondemand_ids[@]}" -ge 5 ]; then
                        break
                    fi
                done
            fi

            if [ "${#ondemand_ids[@]}" -eq 0 ] && [ "${#search_symbol_ids[@]}" -gt 0 ]; then
                ondemand_ids=("${search_symbol_ids[@]:0:5}")
            fi

            for ondemand_id in "${ondemand_ids[@]}"; do
                run_query_benchmark "${repo_dir}" "get_sir" "${repo_log_dir}/phase_c_get_sir_ondemand_${ondemand_id}" "${query_base_url}" "${ondemand_id}"
                if [ "${QUERY_BENCH_EXIT_CODE}" -eq 0 ]; then
                    printf '%s\n' "${QUERY_BENCH_LATENCY_MS}" >> "${ondemand_samples_file}"
                fi
            done

            get_sir_cached_p50="$(percentile_from_file "${cached_samples_file}" 50)"
            get_sir_cached_p95="$(percentile_from_file "${cached_samples_file}" 95)"
            get_sir_cached_p99="$(percentile_from_file "${cached_samples_file}" 99)"

            get_sir_ondemand_p50="$(percentile_from_file "${ondemand_samples_file}" 50)"
            get_sir_ondemand_p95="$(percentile_from_file "${ondemand_samples_file}" 95)"
            get_sir_ondemand_p99="$(percentile_from_file "${ondemand_samples_file}" 99)"

            stop_query_server "${query_pid}"
        else
            call_chain_p50="n/a"
            call_chain_p95="n/a"
            call_chain_p99="n/a"
        fi
    fi

    if [ "${repo_status}" = "ok" ] && [ "${repo_tier}" = "medium" ] && [ "${PROVIDER}" != "pass1-only" ]; then
        crash_stdout="${repo_log_dir}/phase_d_crash.stdout.log"
        crash_stderr="${repo_log_dir}/phase_d_crash.stderr.log"

        set +e
        "${AETHER_BIN}" --workspace "${repo_dir}" --index-once --full --print-sir >"${crash_stdout}" 2>"${crash_stderr}" &
        crash_pid=$!
        set -e

        crash_triggered=0
        while kill -0 "${crash_pid}" 2>/dev/null; do
            write_count="$(grep -c '^SIR_STORED' "${crash_stdout}" 2>/dev/null || true)"
            if [ "${write_count}" -ge 50 ]; then
                crash_triggered=1
                break
            fi
            sleep 1
        done

        if [ "${crash_triggered}" -eq 1 ]; then
            kill_and_recover "${crash_pid}" "${repo_dir}" "${repo_log_dir}/phase_d_recovery"
            crash_recovery_time="$(format_seconds_from_ms "${RECOVERY_WALL_MS}")"
            crash_replayed_intents="${RECOVERY_REPLAYED_INTENTS}"
            run_aether_fsck "${repo_dir}" "${repo_log_dir}/phase_d_fsck"
            crash_fsck_status="${FSCK_STATUS}"
        else
            set +e
            wait "${crash_pid}" 2>/dev/null
            set -e
            crash_recovery_time="skipped (fewer than 50 SIR writes observed)"
        fi
    fi

    if [ "${ENABLE_CONCURRENT}" = "1" ] && [ "${repo_status}" = "ok" ]; then
        log_info "Concurrent phase requested for ${repo_name}; baseline script records only primary phases"
    fi

    if [ -z "${resolved_sha}" ] && [ -d "${repo_dir}/.git" ]; then
        resolved_sha="$(git -C "${repo_dir}" rev-parse HEAD)"
    fi
    resolved_sha="${resolved_sha:-unknown}"

    repo_json="$(generate_json_result \
        "name=${repo_name}" \
        "tier=${repo_tier}" \
        "language=${repo_language}" \
        "resolved_sha=${resolved_sha}" \
        "status=${repo_status}" \
        "error=${error_message}" \
        "symbols=${pass1_symbol_count}" \
        "edges=${pass1_edge_count}" \
        "pass1_time=${pass1_time}" \
        "pass1_peak_memory=${pass1_peak_memory}" \
        "pass1_fsck=${pass1_fsck_status}" \
        "pass2_sample_count=${pass2_sample_count}" \
        "pass2_success_rate=${pass2_success_rate}" \
        "pass2_provider_split=${pass2_provider_split}" \
        "pass2_p50=${pass2_p50}" \
        "pass2_p95=${pass2_p95}" \
        "pass2_p99=${pass2_p99}" \
        "pass2_fsck=${pass2_fsck_status}" \
        "lexical_p50=${lexical_p50}" \
        "lexical_p95=${lexical_p95}" \
        "lexical_p99=${lexical_p99}" \
        "call_chain_p50=${call_chain_p50}" \
        "call_chain_p95=${call_chain_p95}" \
        "call_chain_p99=${call_chain_p99}" \
        "get_sir_cached_p50=${get_sir_cached_p50}" \
        "get_sir_cached_p95=${get_sir_cached_p95}" \
        "get_sir_cached_p99=${get_sir_cached_p99}" \
        "get_sir_ondemand_p50=${get_sir_ondemand_p50}" \
        "get_sir_ondemand_p95=${get_sir_ondemand_p95}" \
        "get_sir_ondemand_p99=${get_sir_ondemand_p99}" \
        "crash_recovery_time=${crash_recovery_time}" \
        "crash_replayed_intents=${crash_replayed_intents}" \
        "crash_fsck=${crash_fsck_status}"
    )"

    printf '%s\n' "${repo_json}" >> "${JSONL_RESULTS}"

done

{
    printf '{\n'
    printf '  "generated_at":"%s",\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '  "aether_version":"%s",\n' "$(json_escape "${AETHER_VERSION}")"
    printf '  "aether_commit":"%s",\n' "$(json_escape "${AETHER_COMMIT}")"
    printf '  "system":"%s",\n' "$(json_escape "${SYSTEM_INFO}")"
    printf '  "bench_dir":"%s",\n' "$(json_escape "${BENCH_DIR}")"
    printf '  "storage_backend":"%s",\n' "$(json_escape "${STORAGE_BACKEND}")"
    printf '  "tier":"%s",\n' "$(json_escape "${TIER}")"
    printf '  "provider":"%s",\n' "$(json_escape "$(provider_description)")"
    printf '  "repos":[\n'

    if [ -s "${JSONL_RESULTS}" ]; then
        awk '{
            if (NR > 1) {
                printf ",\n"
            }
            printf "    %s", $0
        } END { printf "\n" }' "${JSONL_RESULTS}"
    fi

    printf '  ]\n'
    printf '}\n'
} > "${REPORT_JSON}"

provider_config_block_file="${RUN_TMP_DIR}/provider_config_block.toml"
write_provider_config_block > "${provider_config_block_file}"

{
    printf '# AETHER Scale Report\n'
    printf 'Generated: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'AETHER Version: %s (commit %s)\n' "${AETHER_VERSION}" "${AETHER_COMMIT}"
    printf 'System: %s\n' "${SYSTEM_INFO}"
    printf 'Bench Dir: %s\n' "${BENCH_DIR}"
    printf 'Storage Backend: %s\n' "${STORAGE_BACKEND}"
    printf 'Provider: %s\n\n' "$(provider_description)"

    printf '## Effective Provider Config\n\n'
    printf '```toml\n'
    cat "${provider_config_block_file}"
    printf '```\n\n'

    printf '## Summary\n'
    printf '| Repo | Symbols | Pass 1 Time | Peak Memory | SIR Rate | fsck |\n'
    printf '|------|---------|-------------|-------------|----------|------|\n'

    while IFS= read -r line; do
        repo_name="$(extract_json_field "${line}" "name")"
        symbols="$(extract_json_field "${line}" "symbols")"
        pass1_time="$(extract_json_field "${line}" "pass1_time")"
        pass1_peak_memory="$(extract_json_field "${line}" "pass1_peak_memory")"
        pass2_success_rate="$(extract_json_field "${line}" "pass2_success_rate")"
        pass1_fsck="$(extract_json_field "${line}" "pass1_fsck")"
        printf '| %s | %s | %s | %s | %s | %s |\n' \
            "${repo_name}" "${symbols}" "${pass1_time}" "${pass1_peak_memory}" "${pass2_success_rate}" "${pass1_fsck}"
    done < "${JSONL_RESULTS}"

    printf '\n## Pass 2 Detail (Sampled First 100 Outcomes)\n'
    printf '| Repo | Provider Split | p50 | p95 | p99 | Success | fsck |\n'
    printf '|------|----------------|-----|-----|-----|---------|------|\n'

    while IFS= read -r line; do
        repo_name="$(extract_json_field "${line}" "name")"
        provider_split="$(extract_json_field "${line}" "pass2_provider_split")"
        p50="$(extract_json_field "${line}" "pass2_p50")"
        p95="$(extract_json_field "${line}" "pass2_p95")"
        p99="$(extract_json_field "${line}" "pass2_p99")"
        success="$(extract_json_field "${line}" "pass2_success_rate")"
        fsck_status="$(extract_json_field "${line}" "pass2_fsck")"
        printf '| %s | %s | %s | %s | %s | %s | %s |\n' \
            "${repo_name}" "${provider_split}" "${p50}" "${p95}" "${p99}" "${success}" "${fsck_status}"
    done < "${JSONL_RESULTS}"

    printf '\n## Query Latency\n'
    printf '| Repo | Lexical (p50/p95/p99 ms) | Call Chain (p50/p95/p99 ms) | get_sir cached (p50/p95/p99 ms) | get_sir on-demand (p50/p95/p99 ms) |\n'
    printf '|------|---------------------------|------------------------------|----------------------------------|-------------------------------------|\n'

    while IFS= read -r line; do
        repo_name="$(extract_json_field "${line}" "name")"
        lexical="$(extract_json_field "${line}" "lexical_p50")/$(extract_json_field "${line}" "lexical_p95")/$(extract_json_field "${line}" "lexical_p99")"
        call_chain="$(extract_json_field "${line}" "call_chain_p50")/$(extract_json_field "${line}" "call_chain_p95")/$(extract_json_field "${line}" "call_chain_p99")"
        cached="$(extract_json_field "${line}" "get_sir_cached_p50")/$(extract_json_field "${line}" "get_sir_cached_p95")/$(extract_json_field "${line}" "get_sir_cached_p99")"
        ondemand="$(extract_json_field "${line}" "get_sir_ondemand_p50")/$(extract_json_field "${line}" "get_sir_ondemand_p95")/$(extract_json_field "${line}" "get_sir_ondemand_p99")"
        printf '| %s | %s | %s | %s | %s |\n' "${repo_name}" "${lexical}" "${call_chain}" "${cached}" "${ondemand}"
    done < "${JSONL_RESULTS}"

    printf '\n## Crash Recovery\n'
    while IFS= read -r line; do
        repo_name="$(extract_json_field "${line}" "name")"
        recovery_time="$(extract_json_field "${line}" "crash_recovery_time")"
        replayed="$(extract_json_field "${line}" "crash_replayed_intents")"
        fsck_status="$(extract_json_field "${line}" "crash_fsck")"
        printf '- %s: recovery=%s, replayed_intents=%s, fsck=%s\n' "${repo_name}" "${recovery_time}" "${replayed}" "${fsck_status}"
    done < "${JSONL_RESULTS}"

    printf '\n## Notes\n'
    printf -- '- Pass 2 metrics are sampled from the first observed outcomes during `--index-once --full --print-sir` and may end with an intentional sampled stop.\n'
    printf -- '- Tiered provider split is marked unavailable because current logs do not expose per-symbol primary vs fallback routing.\n'
    if [ "${STORAGE_BACKEND}" = "windows-9p" ]; then
        printf -- '- Storage uses WSL2 9P bridge (`/mnt/*`); Pass 1 timings are typically slower than native ext4.\n'
    elif [ "${STORAGE_BACKEND}" = "tmpfs" ]; then
        printf -- '- Bench dir is on tmpfs; large tiers may hit RAM pressure or OOM.\n'
    fi

    printf '\n## Errors\n'
    errors_found=0
    while IFS= read -r line; do
        repo_name="$(extract_json_field "${line}" "name")"
        status="$(extract_json_field "${line}" "status")"
        err="$(extract_json_field "${line}" "error")"
        if [ "${status}" != "ok" ]; then
            errors_found=1
            printf -- '- %s: %s\n' "${repo_name}" "${err}"
        fi
    done < "${JSONL_RESULTS}"
    if [ "${errors_found}" -eq 0 ]; then
        printf -- '- none\n'
    fi

} > "${REPORT_MD}"

log_info "JSON report written: ${REPORT_JSON}"
log_info "Markdown report written: ${REPORT_MD}"
printf '%s\n' "${REPORT_MD}"
