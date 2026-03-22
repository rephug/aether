# Codex Prompt: Add Thinking Support to Real-Time Gemini Provider

## Context

The batch Gemini provider (`batch/gemini.rs`) already supports `thinkingConfig.thinkingLevel` via `gemini_thinking_level()`. The real-time `GeminiProvider` (`aether-infer/src/providers/gemini.rs`) does not — it hardcodes a simple request body with no thinking config.

**Goal:** Add a `thinking` field to `GeminiProvider` that optionally injects `thinkingConfig` into the generateContent request body. This enables turbo mode + thinking for live scan/triage.

## Preflight

```bash
git status --porcelain
git pull --ff-only
git worktree add -B feature/gemini-thinking /home/rephu/feature/gemini-thinking
cd /home/rephu/feature/gemini-thinking
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Mandatory Source Inspection

1. Read `crates/aether-infer/src/providers/gemini.rs` entirely. Note:
   - `request_candidate_json_with_prompt` builds the JSON body with `generationConfig`
   - No thinking config exists
   - `GeminiProvider` struct has: `client`, `api_key`, `model`, `api_base`

2. Read `crates/aetherd/src/batch/gemini.rs` lines 24-35 for `gemini_thinking_level()` — the mapping from string to Gemini constant.

3. Read `crates/aether-config/src/inference.rs` to find the `InferenceConfig` struct. Check if a `thinking` field already exists.

4. Read `crates/aetherd/src/sir_pipeline/mod.rs` `SirPipeline::new()` to see how `GeminiProvider` is constructed. Check how to pass the thinking level through.

5. Read `crates/aether-config/src/sir_quality.rs` for `triage_thinking` and `deep_thinking` fields — these are batch-only currently. Note how they're threaded through.

## Implementation

### Step 1: Add `thinking` field to `GeminiProvider`

In `crates/aether-infer/src/providers/gemini.rs`:

- Add `thinking: Option<String>` to the `GeminiProvider` struct
- Update `new()` to accept `thinking: Option<String>` and store it
- Update `from_env_key()` to accept `thinking: Option<String>` and pass it through
- Update the `Clone` and `Debug` impls to include the new field

### Step 2: Inject thinkingConfig into request body

In `request_candidate_json_with_prompt`, modify the `generationConfig` construction:

```rust
let mut gen_config = json!({
    "responseMimeType": "application/json",
    "temperature": 0.0
});

if let Some(ref level) = self.thinking {
    let gemini_level = match level.trim().to_ascii_lowercase().as_str() {
        "low" => Some("LOW"),
        "medium" => Some("MEDIUM"),
        "high" => Some("HIGH"),
        "dynamic" => Some("DYNAMIC"),
        _ => None,
    };
    if let Some(gl) = gemini_level {
        gen_config["thinkingConfig"] = json!({ "thinkingLevel": gl });
    }
}
```

Use this `gen_config` in the request body instead of the hardcoded object.

### Step 3: Add `thinking` field to InferenceConfig

In `crates/aether-config/src/inference.rs`, add to `InferenceConfig`:

```rust
#[serde(default)]
pub thinking: Option<String>,
```

Update the `Default` impl to include `thinking: None`.

### Step 4: Thread thinking through provider construction

In `crates/aether-infer/src/lib.rs` (or wherever `load_provider_from_env_or_mock` is), find where `GeminiProvider::from_env_key()` is called and pass the thinking config through.

Search for all call sites of `GeminiProvider::new()` and `GeminiProvider::from_env_key()` and update them.

Also check the triage/deep pipeline construction in `crates/aetherd/src/indexer.rs` — when constructing SirPipeline for triage/deep passes, the thinking level should come from `sir_quality.triage_thinking` or `sir_quality.deep_thinking` if those fields exist, falling back to `inference.thinking`.

### Step 5: Add triage_thinking / deep_thinking to SirQualityConfig (if not already present)

Check if `triage_thinking` and `deep_thinking` fields exist in `SirQualityConfig`. If not, add them:

```rust
#[serde(default)]
pub triage_thinking: Option<String>,
#[serde(default)]
pub deep_thinking: Option<String>,
```

These may already exist since the batch path uses them. If they do, just make sure the triage/deep pipeline construction reads them.

## Scope Guard

**Crates modified:**
- `crates/aether-infer/src/providers/gemini.rs` — add thinking field + inject into request
- `crates/aether-config/src/inference.rs` — add thinking field to InferenceConfig
- Whatever file constructs GeminiProvider — thread thinking through

**NOT modified:**
- Batch provider (already has thinking)
- CLI (no new flags needed — config-driven)
- Schema

## Validation

```bash
cargo fmt --all --check
cargo clippy -p aether-infer -- -D warnings
cargo clippy -p aether-config -- -D warnings
cargo clippy -p aetherd --features dashboard -- -D warnings
cargo test -p aether-infer
cargo test -p aether-config
cargo test -p aetherd
```

Do NOT run `cargo test --workspace`.

## Commit

```
feat(infer): add thinking support to real-time Gemini provider

Add optional thinkingConfig.thinkingLevel to Gemini generateContent
requests. Controlled via [inference].thinking config field for scan
and [sir_quality].triage_thinking / deep_thinking for quality passes.

Supports LOW, MEDIUM, HIGH, DYNAMIC levels matching the batch provider.
Omitted when set to "off", "none", or empty (preserving current behavior).
```

## Post-fix Cleanup

```bash
git push origin feature/gemini-thinking
```

Create PR via GitHub web UI. After merge:
```bash
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/gemini-thinking
git branch -D feature/gemini-thinking
```

## PR Title

`feat(infer): add thinking support to real-time Gemini provider`
