# Phase 5 - Stage 5.7: Ollama Local SIR Inference

## Purpose
Enable fully offline SIR generation by formalizing Ollama as a supported inference provider with a recommended model, setup tooling, and quality safeguards. Today AETHER requires a Gemini API key for useful SIR generation — without one, only the `mock` provider is available, which produces placeholder summaries. This stage makes "Full Local" a real deployment option: no internet, no API key, no cloud dependency. Additionally, documents Ollama Cloud as a zero-code-change alternative for users who want cloud-quality inference without managing a Gemini API key.

## Current implementation (what exists)
- `InferenceProvider` trait with three implementations: `MockProvider`, `GeminiProvider`, `Qwen3LocalProvider`
- `Qwen3LocalProvider` already talks to Ollama's HTTP API format at `localhost:11434`
- Config: `[inference] provider = "qwen3_local"` activates the local provider
- SIR generation prompt in `build_strict_json_prompt()` sends symbol text + context and expects strict JSON
- Retry logic (`run_sir_parse_validation_retries`) handles malformed JSON responses (2 retries), but retries re-ask from scratch without feeding the validation error back to the model
- Ollama request body does not set `temperature` — defaults to model's own setting (typically 0.7-1.0), which is too high for deterministic structured output
- No setup tooling, no model recommendation, no quality validation, no documentation for end users
- CLI now supports subcommands alongside existing flags (refactored in Stage 5.6 for `init-agent`)

## Target implementation
- `aether setup-local` CLI subcommand that validates Ollama installation, pulls the recommended model, and tests connectivity
- Recommended model documented and embedded in constants: `Qwen2.5-Coder-7B-Instruct` (Q4_K_M quantization)
- Low temperature (`0.1`) enforced in Ollama requests for deterministic structured JSON output
- Enhanced retry loop: on JSON parse/validation failure, feed the validation error back to the model in the retry prompt
- Quality floor: warn when SIR confidence is consistently below threshold, suggesting the model may be too small
- README section on local inference setup
- No changes to the `InferenceProvider` trait — enhancements are internal to `Qwen3LocalProvider` and the retry helper

## In scope
- Add `setup-local` subcommand to `aetherd` CLI (follows the subcommand pattern established in Stage 5.6 for `init-agent`)
- Add recommended model constants in `crates/aether-core` or `crates/aether-config`:
  ```rust
  pub const RECOMMENDED_OLLAMA_MODEL: &str = "qwen2.5-coder:7b-instruct-q4_K_M";
  pub const OLLAMA_DEFAULT_ENDPOINT: &str = "http://127.0.0.1:11434";
  pub const SIR_QUALITY_FLOOR_CONFIDENCE: f32 = 0.3;
  pub const SIR_QUALITY_FLOOR_WINDOW: usize = 10; // check last N SIR results
  pub const OLLAMA_SIR_TEMPERATURE: f32 = 0.1;    // low temp for deterministic JSON
  ```
- Set `temperature: 0.1` in `Qwen3LocalProvider::request_candidate_json()` Ollama request body:
  - Add `"temperature": OLLAMA_SIR_TEMPERATURE` to the JSON body sent to `/api/generate`
  - Only applies to the local provider — Gemini provider is unaffected
  - Rationale: SIR generation needs deterministic structured output, not creative variation
- Enhance `run_sir_parse_validation_retries` in `crates/aether-infer/src/lib.rs`:
  - On parse/validation failure, construct a retry prompt that includes the previous invalid output and the specific error
  - Retry prompt format: `"Your previous response was invalid JSON. Error: {error}. Previous output: {output}. Please respond again with STRICT JSON only..."`
  - Falls back to the original from-scratch retry if the error-feedback retry also fails
  - This significantly improves JSON reliability on smaller models where the first attempt is close but malformed
- `setup-local` subcommand workflow:
  1. Check if Ollama is reachable at configured endpoint (HTTP GET to `/api/tags`)
  2. Check if recommended model is already pulled (parse model list response)
  3. If not pulled, offer to pull it (HTTP POST to `/api/pull` with streaming progress)
  4. Run a test SIR generation against a small code snippet to validate output quality
  5. Update `.aether/config.toml` to set `provider = "qwen3_local"` (with user confirmation)
- Quality floor monitoring in SIR pipeline:
  - Track rolling confidence average over last N SIR generations
  - When rolling average drops below `SIR_QUALITY_FLOOR_CONFIDENCE`, emit `tracing::warn!`
  - Warning is advisory only — does not block SIR generation
  - Warning text: "SIR quality is low (avg confidence {avg:.2}). Consider using a larger model or switching to Gemini."
- README section on local inference setup
- Note in README that `qwen3_local` config value works with any Ollama-compatible endpoint/model

## Out of scope
- Renaming `qwen3_local` provider to `ollama` (would break existing configs; document instead)
- Model-size-aware prompt tuning (rely on retries; 8B floor is sufficient)
- Profile shortcut (`profile = "local"`) — keep inference and embeddings config independent
- GPU acceleration for Ollama (user configures Ollama independently; AETHER just talks HTTP)
- Bundling or distributing Ollama itself
- Supporting non-Ollama local inference servers (llama.cpp, vLLM, etc.) — they can work if API-compatible but are not tested/documented
- Automatic model selection based on available hardware
- CLI parser refactor (already done in Stage 5.6 — just add a new subcommand variant)

## Locked decisions

### 36. Recommended local model: Qwen2.5-Coder-7B-Instruct (Q4_K_M)
Selected over Qwen3-8B (general purpose, not code-specialized) and DeepSeek-R1-0528-Qwen3-8B (reasoning-focused, unnecessary latency overhead for structured extraction). Qwen2.5-Coder-7B-Instruct is purpose-built for code understanding with strong instruction-following for reliable JSON output. Q4_K_M quantization keeps memory at ~5-6GB RAM.

### 37. Minimum model floor: 8B parameters
Models below ~7-8B parameters produce unreliable SIR — frequent JSON parse failures, hallucinated side effects, low confidence scores. AETHER does not block smaller models but warns when quality degrades. The quality floor is advisory, not enforced.

### 38. Setup-local as CLI subcommand
`aether setup-local` walks the user through Ollama validation, model pull, and config update. This follows the subcommand pattern established in Stage 5.6 (`init-agent`). `setup-local` is added as a new variant in the existing `Commands` enum — no CLI parser refactor needed (that was done in 5.6).

### 39. Quality floor via rolling confidence monitoring
SIR pipeline tracks a rolling average of confidence scores. When average drops below 0.3 over the last 10 generations, a warning is logged. This catches cases where a too-small model is producing garbage SIR without blocking users who want to experiment.

### 40. Low temperature for SIR generation (0.1)
SIR generation is a structured extraction task, not a creative one. Low temperature produces more consistent JSON formatting, more deterministic field values, and fewer hallucinated side effects. Set at 0.1 rather than 0.0 to allow minimal variation (pure greedy decoding can sometimes get stuck in repetition loops on smaller models).

### 41. Retry with validation error feedback
When a model produces invalid JSON on the first attempt, the retry includes the previous output and the specific validation error. This gives the model a chance to self-correct ("you forgot the error_modes field") rather than regenerating from scratch. Especially effective on 7-8B models where the first attempt is often structurally close but has a minor issue.

### Models to watch (not decisions — future evaluation)
- **Seed-Coder-8B-Instruct** (ByteDance): Newer code-specialized model claiming SOTA among ~8B models on SWE-bench. No first-party Ollama support yet. Worth head-to-head testing against Qwen2.5-Coder using the SIR Quality Benchmark (see `sir_quality_benchmark.md`). If it wins convincingly, update the recommended model constant.
- **Nanbeige4.1-3B**: Impressive general-purpose 3B model that outperforms much larger models on reasoning benchmarks. Not code-specialized, and below the 8B quality floor — but if a code fine-tune appears (analogous to Qwen2.5-Coder being a code fine-tune of Qwen2.5), it could be an interesting lightweight option for constrained hardware. Phase 6+ consideration.

### Note: Ollama Cloud models (zero code changes)
Ollama v0.12+ supports cloud-hosted models that are accessed through the same local API (`localhost:11434`). AETHER's `Qwen3LocalProvider` works with cloud models out of the box — users just change the model name (e.g., `qwen3-coder:480b-cloud`). This provides a third deployment profile beyond Gemini and local: cloud inference via Ollama with no separate API key management, access to 480B+ code-specialized models, and Ollama's privacy policy (no prompt/output retention). No new provider code is needed — just README documentation.

## Implementation notes

### CLI command: `aether setup-local`

```
aetherd --workspace . setup-local [--endpoint <url>] [--model <n>] [--skip-pull] [--skip-config]
```

- Default endpoint: `http://127.0.0.1:11434`
- Default model: `qwen2.5-coder:7b-instruct-q4_K_M` (from constant)
- `--skip-pull`: skip model download (user already has a model)
- `--skip-config`: skip config update (user wants to configure manually)
- Exit codes: 0 = success, 1 = Ollama not reachable, 2 = model pull failed, 3 = test generation failed

### Adding the subcommand

Stage 5.6 introduced the `Commands` enum with `InitAgent`. This stage adds `SetupLocal`:

```rust
#[derive(Subcommand)]
enum Commands {
    /// Generate agent configuration files for AI coding agents
    InitAgent { /* ... from 5.6 ... */ },

    /// Set up local Ollama inference for offline SIR generation
    SetupLocal {
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        endpoint: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        skip_pull: bool,
        #[arg(long)]
        skip_config: bool,
    },
}
```

### Setup-local workflow

```
$ aetherd --workspace . setup-local

[1/4] Checking Ollama at http://127.0.0.1:11434... ✔ Ollama v0.5.x found
[2/4] Checking for model qwen2.5-coder:7b-instruct-q4_K_M...
      Model not found. Pulling (~4.4 GB download)...
      ████████████████████████████ 100% (4.4 GB)
      ✔ Model pulled successfully
[3/4] Testing SIR generation with sample code...
      ✔ Generated valid SIR (confidence: 0.85)
[4/4] Updating .aether/config.toml...
      Set [inference] provider = "qwen3_local"
      Set [inference] model = "qwen2.5-coder:7b-instruct-q4_K_M"
      ✔ Config updated

Local inference is ready. Run `aetherd --workspace . --index-once` to re-index with local SIR.
```

### Ollama API usage

Ollama exposes a simple REST API. AETHER uses three endpoints:

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/tags` | GET | List installed models (check if model exists) |
| `/api/pull` | POST | Pull a model (streaming progress) |
| `/api/generate` | POST | Generate completion (already used by `Qwen3LocalProvider`) |

The `/api/pull` endpoint streams JSON objects with `status` and `completed`/`total` fields for progress display:
```json
{"status": "pulling manifest"}
{"status": "downloading", "completed": 1234567, "total": 4400000000}
{"status": "success"}
```

AETHER reads these line-by-line and prints a progress bar to stderr.

### Temperature enforcement

In `Qwen3LocalProvider::request_candidate_json()`, add `temperature` to the request body:

```rust
let body = json!({
    "model": self.model,
    "prompt": build_strict_json_prompt(symbol_text, context),
    "stream": false,
    "format": "json",
    "options": {
        "temperature": OLLAMA_SIR_TEMPERATURE  // 0.1
    }
});
```

Note: Ollama's `/api/generate` accepts model options inside an `"options"` object, not at the top level. The `"format": "json"` flag is already set and constrains output to valid JSON tokens — low temperature further reduces structural variation.

### Enhanced retry with error feedback

Current retry in `run_sir_parse_validation_retries` calls the generation function from scratch on each attempt. Enhanced version:

```rust
async fn run_sir_parse_validation_retries<F, Fut>(
    max_retries: usize,
    generate_fn: F,
    feedback_fn: Option<impl Fn(&str, &str) -> Pin<Box<dyn Future<Output = Result<String, InferError>> + Send>>>,
) -> Result<SirAnnotation, InferError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<String, InferError>>,
{
    let mut last_output = String::new();
    let mut last_error = String::new();

    for attempt in 0..=max_retries {
        // First attempt or fallback: generate from scratch
        // Subsequent attempts with feedback_fn: include previous error
        let candidate = if attempt > 0 && !last_output.is_empty() {
            if let Some(ref feedback) = feedback_fn {
                match feedback(&last_output, &last_error).await {
                    Ok(c) => c,
                    Err(_) => generate_fn().await?,  // fallback to from-scratch
                }
            } else {
                generate_fn().await?
            }
        } else {
            generate_fn().await?
        };

        match parse_and_validate_sir(&candidate) {
            Ok(sir) => return Ok(sir),
            Err(e) => {
                last_output = candidate;
                last_error = e.to_string();
                if attempt == max_retries {
                    return Err(InferError::ParseValidationExhausted(last_error));
                }
            }
        }
    }
    unreachable!()
}
```

The feedback prompt sent to Ollama on retry:
```rust
fn build_retry_prompt(original_prompt: &str, previous_output: &str, error: &str) -> String {
    format!(
        "{original_prompt}\n\n\
         Your previous response was invalid. Error: {error}\n\
         Previous output: {previous_output}\n\n\
         Please respond again with STRICT JSON only, fixing the error above."
    )
}
```

This is implemented in `Qwen3LocalProvider` only — `GeminiProvider` keeps its existing retry behavior (Gemini's JSON mode is reliable enough that feedback rarely helps).

### Quality floor implementation

Add to `crates/aetherd/src/quality.rs`:

```rust
pub struct SirQualityMonitor {
    recent_confidences: VecDeque<f32>,
    window_size: usize,
    floor: f32,
    warned: bool,
}

impl SirQualityMonitor {
    pub fn new() -> Self {
        Self {
            recent_confidences: VecDeque::new(),
            window_size: SIR_QUALITY_FLOOR_WINDOW,
            floor: SIR_QUALITY_FLOOR_CONFIDENCE,
            warned: false,
        }
    }

    pub fn record(&mut self, confidence: f32) {
        self.recent_confidences.push_back(confidence);
        if self.recent_confidences.len() > self.window_size {
            self.recent_confidences.pop_front();
        }
        self.check_floor();
    }

    fn check_floor(&mut self) {
        if self.recent_confidences.len() < self.window_size {
            return; // not enough data yet
        }
        let avg: f32 = self.recent_confidences.iter().sum::<f32>()
            / self.recent_confidences.len() as f32;
        if avg < self.floor && !self.warned {
            tracing::warn!(
                avg_confidence = avg,
                window = self.window_size,
                "SIR quality is low (avg confidence {avg:.2}). \
                 Consider using a larger model or switching to Gemini."
            );
            self.warned = true;
        } else if avg >= self.floor {
            self.warned = false; // reset if quality recovers
        }
    }
}
```

The monitor lives in the SIR pipeline and gets called after every successful SIR generation. It only warns once per quality dip (resets if quality recovers).

### Test SIR generation snippet

The `setup-local` command uses a small, deterministic test snippet for validation:

```rust
const TEST_SNIPPET: &str = r#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
```

The test checks:
1. Response is valid JSON
2. Response parses into `SirAnnotation`
3. `intent` field is non-empty
4. `confidence` is in [0.0, 1.0]

If the test fails after retries, `setup-local` exits with code 3 and suggests the user try a different model.

### README section

Add after "Agent Integration" section:

```markdown
## Local Inference (Offline Mode)

AETHER can generate SIR summaries entirely offline using a local LLM
via Ollama. No API key or internet connection required.

### Quick Setup

```bash
# 1. Install Ollama (https://ollama.com/download)
# 2. Run the guided setup
aetherd --workspace . setup-local
```

This pulls the recommended model (`qwen2.5-coder:7b-instruct-q4_K_M`,
~4.4 GB) and configures AETHER to use it.

### Manual Setup

If you prefer to configure manually:

```bash
# Pull any code-capable model
ollama pull qwen2.5-coder:7b-instruct-q4_K_M

# Edit .aether/config.toml
[inference]
provider = "qwen3_local"
model = "qwen2.5-coder:7b-instruct-q4_K_M"
endpoint = "http://127.0.0.1:11434"
```

### Notes
- The `qwen3_local` provider works with any Ollama-compatible model
  and endpoint — the name is historical
- Recommended minimum: 7-8B parameter model for reliable SIR quality
- Smaller models will work but may produce lower-quality summaries;
  AETHER warns when quality degrades
- Local inference is slower than Gemini API (~5-15 seconds per symbol
  on CPU vs ~200-500ms via API)
- Memory requirement: ~5-6GB RAM for the recommended model at Q4_K_M

### Ollama Cloud Models (no API key, no local GPU)

Ollama offers cloud-hosted models that work through the same local
Ollama interface — no code changes, no separate API key. This gives
you access to much larger models (480B+) without local hardware:

```bash
# Sign in to Ollama (one-time)
ollama login

# Pull a cloud model
ollama pull qwen3-coder:480b-cloud

# Edit .aether/config.toml
[inference]
provider = "qwen3_local"
model = "qwen3-coder:480b-cloud"
endpoint = "http://127.0.0.1:11434"
```

Cloud models produce higher-quality SIR than local 8B models and
require no GPU. Ollama's free tier covers light usage; paid plans
support heavier workloads. See https://ollama.com/cloud for details.
```

## Edge cases

| Scenario | Behavior |
|----------|----------|
| Ollama not installed / not running | `setup-local` exits with code 1, prints install instructions URL |
| Ollama running but no network (can't pull) | Exit code 2, suggest `ollama pull` manually on a connected machine |
| Model already pulled | Skip pull step, proceed to test |
| Test SIR generation fails (bad JSON) | Retry up to 3 times, then exit code 3 with model suggestion |
| Test SIR generation fails (timeout) | Exit code 3, suggest checking Ollama logs and available RAM |
| User passes `--model` with a small model (e.g., 1B) | Allow it, quality floor will warn during actual indexing |
| `.aether/config.toml` doesn't exist | Create it with local provider defaults |
| `.aether/config.toml` already has `provider = "qwen3_local"` | Skip config update, note already configured |
| Ollama endpoint returns non-JSON | Exit code 1, suggest checking the endpoint URL |
| Model pull interrupted | Ollama handles resume on retry; suggest re-running `setup-local` |
| Quality floor triggered during indexing | Log warning once per dip, continue indexing |
| User switches from local back to Gemini | Quality monitor resets — only tracks current provider's output |
| Ollama running on non-default port | `--endpoint` flag overrides default; config stores the override |
| Retry feedback prompt exceeds model context | Truncate previous output to last 500 chars — enough for error context without blowing context window |
| Model ignores temperature setting | Some Ollama model configs override temperature — AETHER's setting is best-effort, not guaranteed |
| GeminiProvider retry behavior | Unchanged — Gemini's JSON mode is reliable enough that error feedback is unnecessary |
| Ollama cloud model used (`*-cloud` suffix) | Works transparently — same API, same request format, same AETHER code path |
| Ollama cloud model but user not signed in | Ollama returns auth error — AETHER surfaces the HTTP error, user runs `ollama login` |
| Ollama cloud model with `setup-local` | Pull step works for cloud models; test SIR generation works; config updated normally |

## Pass criteria
1. `aetherd --workspace . setup-local` checks Ollama connectivity and reports success/failure with clear messages.
2. `aetherd --workspace . setup-local` pulls the recommended model when not present (with progress output).
3. `aetherd --workspace . setup-local` runs a test SIR generation and validates the response.
4. `aetherd --workspace . setup-local` updates `.aether/config.toml` with local provider settings.
5. `--skip-pull` and `--skip-config` flags work as documented.
6. `RECOMMENDED_OLLAMA_MODEL` constant exists in crate code.
7. `SirQualityMonitor` tracks rolling confidence and warns when average drops below floor.
8. Quality warning fires only once per dip and resets when quality recovers.
9. `Qwen3LocalProvider` sends `temperature: 0.1` in Ollama request options.
10. Retry loop feeds validation error + previous output back to the model on retry attempts.
11. Retry falls back to from-scratch generation if error-feedback retry also fails.
12. README.md has a "Local Inference" section with setup instructions including Ollama Cloud models subsection.
13. Existing `Qwen3LocalProvider` behavior is unchanged for non-temperature fields — no regressions in mock or Gemini paths.
14. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

NOTE ON CLI ARCHITECTURE: Stage 5.6 refactored the CLI to support subcommands
alongside existing flags. The `Commands` enum already has `InitAgent`. Add
`SetupLocal` as a new variant in the same enum — do NOT restructure the CLI
parser. Follow the same pattern established in 5.6.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_7_ollama_local_sir.md (this file)
- crates/aether-infer/src/lib.rs (InferenceProvider trait, Qwen3LocalProvider, build_strict_json_prompt, run_sir_parse_validation_retries)
- crates/aetherd/src/main.rs (CLI entry point — has Commands enum from 5.6 with InitAgent variant)
- crates/aetherd/src/sir_pipeline.rs (SIR generation pipeline)
- crates/aether-config/src/lib.rs (config loading, provider kinds)
- README.md (for adding Local Inference section)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-7-ollama-local-sir off main.
3) Create worktree ../aether-phase5-stage5-7-ollama for that branch and switch into it.
4) Add constants in crates/aether-config/src/lib.rs (or crates/aether-core):
   - RECOMMENDED_OLLAMA_MODEL = "qwen2.5-coder:7b-instruct-q4_K_M"
   - OLLAMA_DEFAULT_ENDPOINT = "http://127.0.0.1:11434"
   - SIR_QUALITY_FLOOR_CONFIDENCE = 0.3
   - SIR_QUALITY_FLOOR_WINDOW = 10
   - OLLAMA_SIR_TEMPERATURE = 0.1
5) In crates/aether-infer/src/lib.rs, update Qwen3LocalProvider::request_candidate_json():
   - Add "options": {"temperature": OLLAMA_SIR_TEMPERATURE} to the Ollama request JSON body
   - The "options" object is where Ollama accepts model parameters — do NOT put temperature at the top level
6) In crates/aether-infer/src/lib.rs, enhance run_sir_parse_validation_retries():
   - On parse/validation failure, construct a retry prompt that includes the previous output and the error message
   - Retry prompt: append to the original prompt: "Your previous response was invalid. Error: {error}. Previous output: {output}. Please respond again with STRICT JSON only, fixing the error above."
   - If the error-feedback retry also fails, fall back to a from-scratch retry (existing behavior)
   - This only applies to the Qwen3LocalProvider path — GeminiProvider retries keep existing behavior
7) Create SirQualityMonitor in crates/aetherd/src/quality.rs:
   - VecDeque-based rolling confidence tracker
   - Warns via tracing::warn when rolling average drops below floor
   - Warns once per dip, resets when quality recovers
8) Wire SirQualityMonitor into the SIR pipeline in sir_pipeline.rs:
   - After each successful SIR generation, call monitor.record(sir.confidence)
   - Monitor is created once per indexing session
9) Create setup-local module at crates/aetherd/src/setup_local.rs:
   - Step 1: HTTP GET to {endpoint}/api/tags — check Ollama is reachable
   - Step 2: Parse model list, check if recommended model is present
   - Step 3: If missing, HTTP POST to {endpoint}/api/pull with streaming progress
   - Step 4: Test SIR generation with a small code snippet, validate JSON output
   - Step 5: Update .aether/config.toml with provider = "qwen3_local" and model
   - Support --endpoint, --model, --skip-pull, --skip-config flags
   - Exit codes: 0 success, 1 Ollama unreachable, 2 pull failed, 3 test failed
10) Add `SetupLocal` variant to the existing `Commands` enum in main.rs, wired to the setup module.
11) Add README.md "Local Inference (Offline Mode)" section after "Agent Integration".
    Include subsections for: Quick Setup, Manual Setup, Notes, and Ollama Cloud Models.
    The Ollama Cloud section explains that cloud models work through the same local API
    with no code changes — users just change the model name (e.g., qwen3-coder:480b-cloud).
12) Add tests:
    - SirQualityMonitor warns after N low-confidence results
    - SirQualityMonitor resets warning when quality recovers
    - SirQualityMonitor does not warn before window is full
    - Qwen3LocalProvider request body includes "options.temperature" = 0.1
    - Retry logic sends error feedback on second attempt (mock the provider to return invalid JSON once, then valid)
    - setup-local module: test config update logic (mock filesystem)
    - setup-local module: test model list parsing from mock /api/tags response
    - Test SIR validation logic for the test snippet
13) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
14) Commit with message: "Add Ollama local SIR inference with setup-local CLI and quality monitoring".
```

## Expected commit
`Add Ollama local SIR inference with setup-local CLI, quality monitoring, low-temperature generation, and retry error feedback`
