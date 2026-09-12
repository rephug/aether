#!/usr/bin/env bash
# scan_all.sh — run /scan across crates in parallel Claude Code sessions (Phase WF.0).
#
# Usage: scripts/scan_all.sh [BATCH_SIZE=100] [MAX_PARALLEL=4] [crate ...]
#
# Preflight: stops any running aetherd / aether-mcp (SurrealKV holds an exclusive lock),
# checks the index exists, and counts [MOCK] / low-confidence SIRs before and after.
set -euo pipefail

BATCH_SIZE="${1:-100}"
MAX_PARALLEL="${2:-4}"
shift $(( $# >= 2 ? 2 : $# )) || true
if [ "$#" -gt 0 ]; then
  CRATES=("$@")
else
  CRATES=(aether-core aether-config aether-parse aether-store aether-infer aetherd \
          aether-mcp aether-analysis aether-health aether-memory aether-lsp \
          aether-document aether-dashboard)
fi

WORKSPACE="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$WORKSPACE"
DB=".aether/meta.sqlite"
LOG_DIR=".aether/scan_logs"
STAMP="$(date +%Y%m%d_%H%M%S)"
MASTER_LOG="$LOG_DIR/scan_${STAMP}.log"
mkdir -p "$LOG_DIR"

log() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" | tee -a "$MASTER_LOG"; }

# The daemon and MCP server hold the graph store lock; scanning runs through claude -p.
pkill -f aetherd 2>/dev/null || true
pkill -f aether-mcp 2>/dev/null || true

if [ ! -f "$DB" ]; then
  echo "error: $DB not found. Index first with:" >&2
  echo "  aetherd --workspace . --index-once --inference-provider mock" >&2
  exit 1
fi
if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "error: sqlite3 is required to count scan targets" >&2
  exit 1
fi
if ! command -v claude >/dev/null 2>&1; then
  echo "error: the claude CLI is required (each crate runs as: claude -p \"/scan <crate> $BATCH_SIZE\")" >&2
  exit 1
fi

count_targets() {
  sqlite3 "$DB" "SELECT COUNT(*) FROM sir WHERE sir_json LIKE '%\"intent\":\"[MOCK]%' OR json_extract(sir_json, '\$.confidence') < 0.2;"
}

BEFORE="$(count_targets)"
log "scan targets before: $BEFORE ([MOCK] or confidence < 0.2)"
if [ "$BEFORE" = "0" ]; then
  log "nothing to scan; run ./enrich_all.sh to deepen existing SIRs"
  exit 0
fi

log "scanning ${#CRATES[@]} crate(s), batch size $BATCH_SIZE, up to $MAX_PARALLEL parallel sessions"
running=0
pids=()
for crate in "${CRATES[@]}"; do
  crate_log="$LOG_DIR/${crate}_${STAMP}.log"
  log "start $crate -> $crate_log"
  ( claude -p "/scan $crate $BATCH_SIZE" > "$crate_log" 2>&1 \
      && echo "done $crate" || echo "FAILED $crate (see $crate_log)" ) | tee -a "$MASTER_LOG" &
  pids+=($!)
  running=$((running + 1))
  if [ "$running" -ge "$MAX_PARALLEL" ]; then
    wait -n
    running=$((running - 1))
  fi
done
wait

AFTER="$(count_targets)"
log "scan targets after: $AFTER (was $BEFORE)"
log "complete: $((BEFORE - AFTER)) symbol(s) scanned; logs in $LOG_DIR"
if [ "$AFTER" != "0" ]; then
  log "rerun scripts/scan_all.sh to continue, then ./enrich_all.sh for depth"
fi
