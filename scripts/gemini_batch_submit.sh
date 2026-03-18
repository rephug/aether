#!/usr/bin/env bash
# DEPRECATED: This script is superseded by native Rust batch providers (Phase 10.7a).
# Kept for reference. Use `aether batch run --provider gemini` instead.
set -euo pipefail

if [[ $# -lt 4 ]]; then
  echo "usage: $0 <input_jsonl> <model> <batch_dir> <poll_interval_secs>" >&2
  exit 64
fi

INPUT_JSONL=$1
MODEL=$2
BATCH_DIR=$3
POLL_INTERVAL_SECS=$4

if [[ ! -f "$INPUT_JSONL" ]]; then
  echo "input JSONL not found: $INPUT_JSONL" >&2
  exit 66
fi

if [[ -z "${GEMINI_API_KEY:-}" ]]; then
  echo "GEMINI_API_KEY is required for Gemini Batch API submission" >&2
  exit 78
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2
  exit 69
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required" >&2
  exit 69
fi

mkdir -p "$BATCH_DIR"

INPUT_ABS=$(realpath "$INPUT_JSONL")
BATCH_DIR_ABS=$(realpath "$BATCH_DIR")
INPUT_BASENAME=$(basename "$INPUT_ABS")
DISPLAY_NAME=${INPUT_BASENAME%.*}
UPLOAD_HEADERS=$(mktemp)
UPLOAD_RESPONSE=$(mktemp)
STATUS_JSON=$(mktemp)
trap 'rm -f "$UPLOAD_HEADERS" "$UPLOAD_RESPONSE" "$STATUS_JSON"' EXIT

NUM_BYTES=$(wc -c <"$INPUT_ABS")

curl "https://generativelanguage.googleapis.com/upload/v1beta/files" \
  -sS \
  -D "$UPLOAD_HEADERS" \
  -H "x-goog-api-key: $GEMINI_API_KEY" \
  -H "X-Goog-Upload-Protocol: resumable" \
  -H "X-Goog-Upload-Command: start" \
  -H "X-Goog-Upload-Header-Content-Length: ${NUM_BYTES}" \
  -H "X-Goog-Upload-Header-Content-Type: application/jsonl" \
  -H "Content-Type: application/json" \
  -d "{\"file\":{\"display_name\":\"${DISPLAY_NAME}\"}}" \
  >/dev/null

UPLOAD_URL=$(awk 'BEGIN{IGNORECASE=1} /^x-goog-upload-url:/ {print $2}' "$UPLOAD_HEADERS" | tr -d '\r')
if [[ -z "$UPLOAD_URL" ]]; then
  echo "failed to obtain Gemini resumable upload URL" >&2
  exit 70
fi

curl "$UPLOAD_URL" \
  -sS \
  -H "Content-Length: ${NUM_BYTES}" \
  -H "X-Goog-Upload-Offset: 0" \
  -H "X-Goog-Upload-Command: upload, finalize" \
  --data-binary "@${INPUT_ABS}" \
  >"$UPLOAD_RESPONSE"

FILE_NAME=$(jq -r '.file.name // .name // empty' "$UPLOAD_RESPONSE")
if [[ -z "$FILE_NAME" ]]; then
  echo "failed to parse uploaded Gemini file name" >&2
  cat "$UPLOAD_RESPONSE" >&2
  exit 70
fi

CREATE_JSON=$(jq -n \
  --arg display_name "$DISPLAY_NAME" \
  --arg file_name "$FILE_NAME" \
  '{batch:{display_name:$display_name,input_config:{file_name:$file_name}}}')

CREATE_RESPONSE=$(curl "https://generativelanguage.googleapis.com/v1beta/models/${MODEL}:batchGenerateContent" \
  -sS \
  -X POST \
  -H "x-goog-api-key: $GEMINI_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$CREATE_JSON")

BATCH_NAME=$(printf '%s' "$CREATE_RESPONSE" | jq -r '.batch.name // .name // empty')
if [[ -z "$BATCH_NAME" ]]; then
  echo "failed to parse created Gemini batch name" >&2
  printf '%s\n' "$CREATE_RESPONSE" >&2
  exit 70
fi

while true; do
  curl "https://generativelanguage.googleapis.com/v1beta/${BATCH_NAME}" \
    -sS \
    -H "x-goog-api-key: $GEMINI_API_KEY" \
    -H "Content-Type: application/json" \
    >"$STATUS_JSON"

  BATCH_STATE=$(jq -r '.metadata.state // .state // empty' "$STATUS_JSON")
  case "$BATCH_STATE" in
    JOB_STATE_SUCCEEDED)
      RESPONSES_FILE=$(jq -r '.response.responsesFile // .output.responsesFile // .dest.fileName // empty' "$STATUS_JSON")
      if [[ -z "$RESPONSES_FILE" ]]; then
        echo "batch succeeded but no responses file was returned" >&2
        cat "$STATUS_JSON" >&2
        exit 70
      fi
      RESULTS_PATH="${BATCH_DIR_ABS}/${DISPLAY_NAME}.results.jsonl"
      curl "https://generativelanguage.googleapis.com/download/v1beta/${RESPONSES_FILE}:download?alt=media" \
        -sS \
        -H "x-goog-api-key: $GEMINI_API_KEY" \
        >"$RESULTS_PATH"
      printf '%s\n' "$RESULTS_PATH"
      exit 0
      ;;
    JOB_STATE_FAILED)
      jq '.error // .' "$STATUS_JSON" >&2
      exit 1
      ;;
    JOB_STATE_CANCELLED|JOB_STATE_EXPIRED)
      cat "$STATUS_JSON" >&2
      exit 1
      ;;
    "")
      echo "batch status response missing state" >&2
      cat "$STATUS_JSON" >&2
      exit 70
      ;;
    *)
      sleep "$POLL_INTERVAL_SECS"
      ;;
  esac
done
