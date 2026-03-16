# Phase 9 — The Beacon

## Stage 9.2 — Configuration UI

### Purpose

Replace TOML file editing with a visual settings interface rendered in the Tauri webview. Every user-facing configuration option in `.aether/config.toml` gets a corresponding UI control — dropdowns, toggles, text inputs, sliders — organized into logical sections. Changes save to the TOML file and hot-reload into the running engine where possible.

### What Problem This Solves

AETHER has accumulated significant configuration surface across 10 phases of development:

- **Inference:** provider (gemini/ollama/openai-compat/mock), model name, endpoint URL, API key env var, rate limits, concurrency
- **Embeddings:** provider (gemini_native/openai-compat/candle/sqlite), model, dimensions, vector backend (lancedb/sqlite)
- **Search:** default mode (lexical/semantic/hybrid), per-language similarity thresholds, reranker toggle
- **Indexing:** watched paths, ignored patterns, debounce interval, max file size
- **Dashboard:** bind address, port, polling interval
- **Graph/Storage:** SurrealDB backend, mirror settings
- **SIR Quality (Phase 8):** triage/deep pass toggles, thresholds, providers, models, concurrency, timeouts
- **Health / Health Score:** health scoring weights, structural health config
- **Batch (Phase 10.1):** batch API models, thinking levels, chunk size, transport
- **Watcher (Phase 10.1):** git triggers (branch switch, pull, merge, rebase), realtime model, debounce
- **Continuous (Phase 10.2):** staleness scoring params, schedule, requeue config, semantic gate threshold
- **Coupling:** co-change mining config
- **Drift:** drift detection config, acknowledgment settings
- **Analysis:** analysis config
- **Verification:** intent verification settings
- **Logging:** level, format, output destination

Asking users to discover, understand, and correctly edit all of these in a TOML file is a barrier to adoption. A settings UI makes every option discoverable with descriptions, validates input before saving, and shows the current effective value.

### Architecture

The settings UI is an HTMX page served by the dashboard HTTP server. It reads the current config via Tauri commands and writes changes back via Tauri commands. No new JavaScript framework — the same HTMX fragment pattern used by the dashboard.

```
Settings Page (HTMX)
    │
    ├── GET /dashboard/frag/settings/{section}  → HTML form fragment
    │       (populated with current values from SharedState.config)
    │
    └── POST /api/v1/settings/{section}         → validates + writes config.toml
            (Tauri command: update_config)        → hot-reloads affected components
```

### In scope

#### Settings sections and controls

**1. Inference**
| Setting | Control | TOML key |
|---------|---------|----------|
| Provider | Dropdown: Gemini / Ollama / OpenAI-compat / Mock | `inference.provider` |
| Model | Text input with suggestions | `inference.model` |
| Endpoint URL | Text input (shown for Ollama/OpenAI-compat) | `inference.endpoint` |
| API Key Env Var | Text input | `inference.api_key_env` |
| Concurrency | Number input (1-24) | `inference.concurrency` |

**2. Embeddings**
| Setting | Control | TOML key |
|---------|---------|----------|
| Provider | Dropdown: Gemini Native / OpenAI-compat / Candle (local) / SQLite | `embeddings.provider` |
| Model | Text input with suggestions | `embeddings.model` |
| API Key Env Var | Text input (shown for cloud providers) | `embeddings.api_key_env` |
| Vector backend | Dropdown: SQLite / LanceDB | `embeddings.vector_backend` |
| Embedding dimensions | Number (read-only, derived from model) | `embeddings.dimensions` |

**3. Search**
| Setting | Control | TOML key |
|---------|---------|----------|
| Default mode | Radio: Lexical / Semantic / Hybrid | `search.default_mode` |
| Reranker enabled | Toggle | `search.reranker` |
| Per-language thresholds | Slider per language (0.0–1.0) | `search.thresholds.*` |

**4. Indexing**
| Setting | Control | TOML key |
|---------|---------|----------|
| Workspace path | Path display + "Change" button (file dialog) | `general.workspace` |
| Ignored patterns | Editable list (add/remove) | `indexing.ignore_patterns` |
| Debounce interval (ms) | Number input | `indexing.debounce_ms` |
| Max file size (bytes) | Number input with KB/MB toggle | `indexing.max_file_size` |

**5. Dashboard**
| Setting | Control | TOML key |
|---------|---------|----------|
| Bind address | Text input | `dashboard.bind` |
| Port | Number input | `dashboard.port` |
| Polling interval (s) | Number input | `dashboard.poll_interval_s` |

**6. SIR Quality (Phase 8)**
| Setting | Control | TOML key |
|---------|---------|----------|
| Triage pass enabled | Toggle | `sir_quality.triage_pass` |
| Triage provider | Dropdown (same as Inference providers) | `sir_quality.triage_provider` |
| Triage model | Text input | `sir_quality.triage_model` |
| Triage concurrency | Number input (1-24) | `sir_quality.triage_concurrency` |
| Triage timeout (s) | Number input | `sir_quality.triage_timeout_secs` |
| Deep pass enabled | Toggle | `sir_quality.deep_pass` |
| Deep provider | Dropdown | `sir_quality.deep_provider` |
| Deep model | Text input | `sir_quality.deep_model` |
| Deep timeout (s) | Number input | `sir_quality.deep_timeout_secs` |

**7. Batch Pipeline (Phase 10.1)**
| Setting | Control | TOML key |
|---------|---------|----------|
| Batch model | Text input | `batch.model` |
| Thinking level | Dropdown: none / low / medium / high | `batch.thinking_level` |
| Chunk size | Number input | `batch.chunk_size` |

**8. Watcher Intelligence (Phase 10.1)**
| Setting | Control | TOML key |
|---------|---------|----------|
| Git branch switch trigger | Toggle | `watcher.git_branch_switch` |
| Git pull trigger | Toggle | `watcher.git_pull` |
| Git merge trigger | Toggle | `watcher.git_merge` |
| Git rebase trigger | Toggle | `watcher.git_rebase` |
| Realtime model | Text input | `watcher.realtime_model` |
| Debounce (ms) | Number input | `watcher.debounce_ms` |

**9. Continuous Monitor (Phase 10.2)**
| Setting | Control | TOML key |
|---------|---------|----------|
| Enabled | Toggle | `continuous.enabled` |
| Staleness threshold | Slider (0.0–1.0) | `continuous.staleness_threshold` |
| Semantic gate threshold | Slider (0.0–1.0) | `continuous.semantic_gate` |
| Requeue max per cycle | Number input | `continuous.requeue_max` |

**10. Health & Analysis**
| Setting | Control | TOML key |
|---------|---------|----------|
| Semantic rescue threshold | Slider (0.0–1.0) | `planner.semantic_rescue_threshold` |
| Health scoring weights | Three sliders (structural/git/semantic) summing to 1.0 | `health.*` |
| Coupling co-change threshold | Slider (0.0–1.0) | `coupling.threshold` |
| Drift detection enabled | Toggle | `drift.enabled` |

**11. Advanced**
| Setting | Control | TOML key |
|---------|---------|----------|
| Log level | Dropdown: error/warn/info/debug/trace | `general.log_level` |
| SurrealDB path | Text input (read-only for embedded) | `graph.path` |
| MCP transport | Dropdown: stdio / HTTP | `mcp.transport` |
| MCP HTTP bind | Text input (shown for HTTP only) | `mcp.bind` |

#### Tauri commands

```rust
#[tauri::command]
async fn get_config_section(section: String) -> Result<serde_json::Value, String>;

#[tauri::command]
async fn update_config_section(
    section: String,
    values: serde_json::Value,
) -> Result<ConfigUpdateResult, String>;

#[tauri::command]
async fn validate_config_value(
    key: String,
    value: String,
) -> Result<ValidationResult, String>;

#[tauri::command]
async fn reset_section_to_defaults(section: String) -> Result<(), String>;
```

#### Hot-reload behavior

After saving config changes:
| Component | Hot-reload? | Notes |
|-----------|-------------|-------|
| Log level | ✅ Immediate | `tracing` subscriber reload |
| Search thresholds | ✅ Immediate | In-memory config swap |
| Reranker toggle | ✅ Immediate | Pipeline flag |
| Debounce interval | ✅ Immediate | Watcher parameter |
| Continuous monitor params | ✅ Immediate | Next cycle picks up changes |
| Watcher git triggers | ✅ Immediate | Toggle flags in memory |
| Inference provider/model | ⚠️ Next request | New provider created lazily |
| SIR quality triage/deep settings | ⚠️ Next pass | Applied on next indexing run |
| Batch pipeline settings | ⚠️ Next batch | Applied on next `batch run` |
| Health scoring weights | ⚠️ Next score | Applied on next `health-score` |
| Embedding provider | ❌ Restart required | Candle model load is expensive |
| Dashboard bind/port | ❌ Restart required | Cannot rebind listener |
| Workspace path | ❌ Restart required | Full re-init of SharedState |

Settings that require restart show a non-blocking banner: "Restart AETHER to apply changes to [embedding provider]." With a "Restart Now" button.

### Out of scope

- Per-project configuration profiles (one config per workspace for now)
- Config import/export
- Config sync across machines
- Undo/redo for config changes

### Implementation Notes

#### HTMX form pattern

Each settings section is an HTMX form fragment:

```html
<!-- GET /dashboard/frag/settings/inference -->
<form hx-post="/api/v1/settings/inference"
      hx-target="#settings-status"
      hx-swap="innerHTML">

  <label>Provider</label>
  <select name="provider" hx-get="/dashboard/frag/settings/inference/provider-options"
          hx-target="#provider-specific" hx-trigger="change">
    <option value="gemini" selected>Gemini</option>
    <option value="ollama">Ollama (Local)</option>
    <option value="mock">Mock (Testing)</option>
  </select>

  <div id="provider-specific">
    <!-- HTMX swaps in provider-specific fields -->
  </div>

  <button type="submit">Save</button>
  <button type="button" hx-post="/api/v1/settings/inference/reset"
          hx-target="closest form" hx-swap="outerHTML">
    Reset to Defaults
  </button>
</form>
```

Conditional fields (e.g., Ollama endpoint only shown when provider=ollama) are handled by HTMX `hx-get` on dropdown change — the server returns the appropriate fields. No client-side JavaScript logic needed.

#### Validation

Server-side validation before write:
- Port numbers: 1–65535, not in use
- File paths: exist and are accessible
- API key env vars: check if env var is set (warn if not, don't block)
- Thresholds: 0.0–1.0 range
- Cost ceiling: non-negative
- Model names: non-empty strings

Validation errors render inline next to the offending field.

#### Config file write strategy

Read → merge → write. Never overwrite the entire file. Use `toml_edit` (preserves comments and formatting) instead of `toml::to_string` (which strips comments).

```toml
# crates/aether-desktop/Cargo.toml (additional deps for this stage)
toml_edit = "0.22"
```

### Pass criteria

1. Settings page is accessible from dashboard navigation and renders all 11 sections.
2. Changing inference provider via dropdown saves to `.aether/config.toml` and is reflected on page reload.
3. Conditional fields (Ollama endpoint, MCP HTTP bind, OpenAI-compat endpoint) show/hide based on dropdown selection.
4. Invalid input (port > 65535, threshold > 1.0) shows inline validation error and does not save.
5. "Reset to Defaults" restores factory values for a section without affecting other sections.
6. Hot-reloadable settings (log level, search thresholds, watcher triggers) take effect without restart.
7. Non-hot-reloadable settings show "Restart required" banner with a working restart button.
8. TOML comments in existing config files are preserved after save (verified by diff).
9. New Phase 10 config sections (batch, watcher, continuous) render correctly with appropriate controls.
10. `cargo fmt --all --check`, `cargo clippy -p aether-desktop -- -D warnings` pass.
11. `cargo test -p aether-desktop` passes (unit tests for config read/write/validate/merge).

### Estimated Claude Code sessions: 2–3
