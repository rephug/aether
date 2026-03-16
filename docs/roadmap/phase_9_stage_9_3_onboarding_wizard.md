# Phase 9 — The Beacon

## Stage 9.3 — Onboarding Wizard

### Purpose

Guide first-time users through AETHER setup with a step-by-step wizard that detects their environment, helps them choose the right configuration, and gets them to a working state without touching a terminal or TOML file. The wizard runs automatically on first launch (no `.aether/` directory detected) and is also accessible from Settings → "Run Setup Again."

### What Problem This Solves

A new user who downloads AETHER Desktop today needs to:
1. Know what a workspace is and where to point it
2. Understand the inference provider options and which requires what
3. Know whether Ollama is installed and running (if they want local inference)
4. Know whether `pdftotext` is available (for legal/finance document processing)
5. Set API keys as environment variables
6. Understand the difference between lexical, semantic, and hybrid search

The wizard automates the discovery part and presents clear choices for the rest. A lawyer installing AETHER Legal should be able to go from download to "seeing their contracts analyzed" in under 5 minutes.

### Wizard Flow

```
┌──────────────────────────────────────────┐
│  Step 1: Welcome                          │
│  "AETHER understands your codebase        │
│   (and documents). Let's get started."    │
│                                [Next →]   │
└──────────────────────────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  Step 2: Choose Workspace                 │
│  [Browse...] /home/user/projects/myapp    │
│                                           │
│  Detected: 47 .rs files, 23 .ts files    │
│  Languages: Rust, TypeScript              │
│                         [← Back] [Next →] │
└──────────────────────────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  Step 3: Environment Check                │
│                                           │
│  ✅ Rust source files detected            │
│  ✅ Git repository found                  │
│  ⚠️ Ollama not detected                  │
│     (needed for fully offline operation)  │
│     [Install Guide] or [Skip]             │
│  ✅ pdftotext available                   │
│  ❌ GEMINI_API_KEY not set               │
│     [Set API Key] or [Use Ollama instead] │
│                         [← Back] [Next →] │
└──────────────────────────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  Step 4: Choose Inference Mode            │
│                                           │
│  ○ Cloud (Gemini Flash) — Best quality,   │
│    requires API key, costs ~$0.01/file    │
│                                           │
│  ○ Local (Ollama) — Fully offline,        │
│    requires 8GB+ RAM, good quality        │
│                                           │
│  ○ Mock (Testing) — No AI, instant,       │
│    placeholder summaries only             │
│                                           │
│  ☐ Enable batch pipeline (nightly)        │
│  ☐ Enable continuous drift monitor        │
│                         [← Back] [Next →] │
└──────────────────────────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  Step 5: Confirm & Start                  │
│                                           │
│  Workspace: /home/user/projects/myapp     │
│  Languages: Rust (143 files),             │
│             TypeScript (87 files)          │
│  Inference: Gemini Flash                  │
│  Batch pipeline: Enabled                  │
│  Continuous monitor: Enabled              │
│  Estimated first index: ~2 minutes        │
│                                           │
│  [← Back]              [Start AETHER →]   │
└──────────────────────────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  Step 6: Indexing Progress                │
│                                           │
│  ████████████░░░░░░░░  67%               │
│  Parsed: 187/230 files                    │
│  SIRs generated: 1,247/1,890 symbols     │
│  Embeddings: 1,100/1,890                  │
│                                           │
│  [Show Dashboard →]  (enabled when done)  │
└──────────────────────────────────────────┘
```

### In scope

#### Environment detection

The wizard probes the system on Step 3:

| Check | Method | Result |
|-------|--------|--------|
| Source files | Recursive file scan with language detection | Count per language |
| Git repo | Check for `.git/` directory | ✅/❌ |
| Ollama running | HTTP GET `http://127.0.0.1:11434/api/version` | ✅/⚠️ with install link |
| Ollama model available | HTTP GET `http://127.0.0.1:11434/api/tags` → check for configured model | ✅/⚠️ with pull command |
| `pdftotext` available | `which pdftotext` or `Command::new("pdftotext").arg("-v")` | ✅/❌ |
| `GEMINI_API_KEY` set | `std::env::var("GEMINI_API_KEY")` | ✅/❌ |
| Available RAM | `sysinfo` crate | Display, warn if < 8GB for local inference |
| Disk space | `fs2::available_space()` | Warn if < 1GB at workspace path |

#### HTMX wizard pattern

Each step is an HTMX fragment. Navigation is server-driven:

```html
<!-- Step 2: Workspace selection -->
<div id="wizard-content">
  <h2>Choose Your Workspace</h2>

  <div class="flex items-center gap-4">
    <input type="text" id="workspace-path" name="workspace_path"
           value="" placeholder="/path/to/your/project" class="flex-1" />
    <button onclick="window.__TAURI__.invoke('pick_directory').then(p => {
      if(p) document.getElementById('workspace-path').value = p;
      htmx.trigger('#workspace-path', 'change');
    })">Browse...</button>
  </div>

  <!-- Auto-detect on path change -->
  <div hx-get="/dashboard/frag/wizard/detect"
       hx-include="#workspace-path"
       hx-trigger="change from:#workspace-path"
       hx-target="#detection-results">
  </div>
  <div id="detection-results"></div>

  <div class="flex justify-between mt-8">
    <button hx-get="/dashboard/frag/wizard/step/1"
            hx-target="#wizard-content">← Back</button>
    <button hx-get="/dashboard/frag/wizard/step/3"
            hx-include="#workspace-path"
            hx-target="#wizard-content">Next →</button>
  </div>
</div>
```

#### Tauri commands for wizard

```rust
#[tauri::command]
async fn pick_directory() -> Result<Option<String>, String>;
// Opens native OS file dialog, returns selected path

#[tauri::command]
async fn detect_environment(workspace_path: String) -> Result<EnvironmentReport, String>;
// Runs all checks from the table above

#[tauri::command]
async fn estimate_index_time(workspace_path: String, provider: String) -> Result<IndexEstimate, String>;
// Counts files, estimates based on provider speed

#[tauri::command]
async fn start_initial_index(workspace_path: String, config: WizardConfig) -> Result<(), String>;
// Writes config.toml, initializes SharedState, starts indexing

#[tauri::command]
async fn get_index_progress() -> Result<IndexProgress, String>;
// Returns current indexing stats for the progress bar
```

#### First-run detection

```rust
fn is_first_run(workspace_path: &Path) -> bool {
    !workspace_path.join(".aether").join("config.toml").exists()
}
```

On Tauri app launch:
- If no workspace is configured (no last-used path stored) → show wizard at Step 1
- If workspace is configured but `.aether/` doesn't exist → show wizard at Step 2 (pre-filled)
- If workspace is configured and `.aether/` exists → go straight to dashboard

Last-used workspace path is stored in Tauri's app data directory (`tauri::api::path::app_data_dir()`), not in the workspace itself.

#### Domain-specific wizard variants

The wizard detects the workspace content type and adjusts its language:

| Detection | Variant | Differences |
|-----------|---------|-------------|
| `.rs`, `.ts`, `.py` files found | **Code** (default) | Standard flow as shown above |
| `.pdf`, `.docx` files + no source code | **Documents** (Legal/Finance) | Step 4 mentions "document analysis" not "code understanding." Skips Git check. Adds "Document type" selection (Contracts / Financial / General). |
| Mixed (code + documents) | **Hybrid** | Shows both code and document counts. Enables all features. |

### Out of scope

- Account creation or login (AETHER is local-first, no accounts)
- Tutorial / guided tour of dashboard features (future enhancement)
- Automatic Ollama installation (link to install guide only)
- Automatic API key provisioning (user provides their own)
- Multi-workspace setup (one workspace per wizard run)

### Pass criteria

1. First launch with no prior config → wizard appears automatically at Step 1.
2. "Browse" button opens native OS file dialog and populates the workspace path.
3. Environment detection correctly identifies: source file counts, Git presence, Ollama status, pdftotext availability, API key status.
4. Selecting "Ollama" when Ollama is not running shows a warning with install guide link.
5. Selecting "Gemini" when API key is not set shows an inline prompt to set it.
6. "Start AETHER" writes a valid `config.toml`, initializes `.aether/`, and begins indexing.
7. Progress bar updates in real-time during initial indexing.
8. After indexing completes, "Show Dashboard" navigates to the main dashboard view.
9. Subsequent launches skip the wizard and go directly to the dashboard.
10. "Run Setup Again" from Settings restarts the wizard with current values pre-filled.
11. `cargo fmt --all --check`, `cargo clippy -p aether-desktop -- -D warnings` pass.
12. `cargo test -p aether-desktop` passes (unit tests for environment detection, first-run logic).

### Estimated Claude Code sessions: 1–2
