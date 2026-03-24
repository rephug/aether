# Claude Code Prompt — CC.7: VS Code Prompt Enhancer Command

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Read the spec first:
- `docs/roadmap/phase_cc7_vscode_enhancer.md`

Then read these source files in order:

**Dashboard HTTP server (add endpoint here):**
- `crates/aether-dashboard/src/lib.rs` (axum router setup, route mounting)
- `crates/aether-dashboard/src/state.rs` (DashboardState — shared state)
- `crates/aether-dashboard/src/fragments/prompts.rs` (existing prompt tab — reference)

**Enhancement core (CC.6 — you call this):**
- `crates/aetherd/src/enhance.rs` (enhance_prompt_core function)

**VS Code extension (modify these):**
- `vscode-extension/src/extension.ts` (activation, command registration)
- `vscode-extension/package.json` (contributes: commands, keybindings, configuration)
- `vscode-extension/tsconfig.json` (check compilation settings)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -B feature/cc7-vscode-enhancer /home/rephu/feature/cc7-vscode-enhancer
cd /home/rephu/feature/cc7-vscode-enhancer
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any is false, STOP and report:

1. `crates/aether-dashboard/src/lib.rs` sets up an axum Router. Check how routes
   are mounted — likely `Router::new().route(...)` or `.nest(...)`. Check if there
   is already an `/api` namespace or if all routes are HTMX fragment routes.

2. `DashboardState` in `state.rs` holds a `shared: Arc<SharedState>` that contains
   the store and config. Check what fields are on `SharedState` — we need store
   access for the enhance function.

3. The `enhance_prompt_core()` function from CC.6 exists in `crates/aetherd/src/enhance.rs`.
   Check its signature: it should take a store reference, optional inference provider,
   the raw prompt, budget, rewrite flag, and offline flag. If the function is
   `pub(crate)`, it needs to be promoted to `pub` or the core logic needs to be
   extracted into a library crate. If it's in `aetherd` (binary crate), the dashboard
   crate can't directly call it. In that case:
   - Option A: Move core logic to `aether-mcp` or a new `aether-enhance` library crate
   - Option B: Reimplement the core assembly in the dashboard route (less ideal)
   - Option C: The dashboard route shells out to `aetherd enhance --output json` (simple but slower)
   
   Decide based on what you find. Option A is best if feasible without large refactoring.
   Option C is acceptable as a first pass.

4. The VS Code extension in `vscode-extension/src/extension.ts` has an `activate()`
   function that registers commands. Check the pattern — likely uses
   `vscode.commands.registerCommand()`.

5. Check `vscode-extension/package.json` for existing `contributes.commands` and
   `contributes.keybindings` to follow the established pattern.

6. Check if the extension already knows the daemon port. Look for any configuration
   for `aether.port` or similar, or if it discovers the port from config files.

## IMPLEMENTATION

### Step 1: Dashboard HTTP endpoint

In `crates/aether-dashboard/src/`:

Create `api.rs` (or add to existing API module if one exists):

```rust
pub async fn enhance_handler(
    State(state): State<Arc<DashboardState>>,
    Json(request): Json<EnhanceApiRequest>,
) -> Result<Json<EnhanceApiResponse>, StatusCode> {
    // Call enhance logic
    // Return JSON response
}

#[derive(Deserialize)]
pub struct EnhanceApiRequest {
    pub prompt: String,
    #[serde(default = "default_budget")]
    pub budget: usize,
    #[serde(default)]
    pub rewrite: bool,
}

#[derive(Serialize)]
pub struct EnhanceApiResponse {
    pub enhanced_prompt: String,
    pub resolved_symbols: Vec<String>,
    pub referenced_files: Vec<String>,
    pub rewrite_used: bool,
    pub token_count: usize,
    pub warnings: Vec<String>,
}
```

Mount the route in `lib.rs`:
```rust
.route("/api/enhance", post(api::enhance_handler))
```

**Important:** If `enhance_prompt_core` is not accessible from the dashboard crate
(because it lives in the `aetherd` binary crate), use Option C from source inspection:
shell out to `aetherd --workspace {workspace} enhance "{prompt}" --output json --budget {budget}`.
Parse the JSON output and return it. This is slower but avoids refactoring the
enhance logic into a library crate in this stage.

### Step 2: VS Code command (`vscode-extension/src/enhancePrompt.ts`)

Create new file with the enhance command:

1. Get text from editor selection, or prompt via input box
2. Read config: daemon port, budget, rewrite preference
3. POST to `http://localhost:{port}/api/enhance`
4. On success: replace selection or open new document
5. On failure: show error message with daemon start instructions

Handle cancellation via the progress API's cancellation token.

### Step 3: Register command in extension.ts

In the `activate()` function:
```typescript
context.subscriptions.push(
    vscode.commands.registerCommand('aether.enhancePrompt', enhancePrompt)
);
```

### Step 4: package.json declarations

Add to `contributes`:
- Command: `aether.enhancePrompt` with title "AETHER: Enhance Prompt"
- Keybinding: `ctrl+shift+e` / `cmd+shift+e` when `editorTextFocus`
- Configuration: `aether.enhance.budget`, `aether.enhance.rewrite`, `aether.daemonPort`

Check for keybinding conflicts with existing VS Code defaults. `Ctrl+Shift+E`
is "Show Explorer" by default. If this conflicts, use `Ctrl+Alt+E` instead
or just register the command without a default keybinding and let users set
their own.

### Step 5: Build + test

```bash
# Rust side
cargo fmt --all --check
cargo clippy -p aether-dashboard -- -D warnings
cargo test -p aether-dashboard

# Extension side
cd vscode-extension
npm install
npm run build
```

Verify the extension compiles. If there's a lint/typecheck script, run it too.

## SCOPE GUARD

**New files:**
- `crates/aether-dashboard/src/api.rs` (or similar — JSON API routes)
- `vscode-extension/src/enhancePrompt.ts`

**Modified files:**
- `crates/aether-dashboard/src/lib.rs` (mount /api routes)
- `crates/aether-dashboard/src/mod.rs` or equivalent (declare api module)
- `vscode-extension/src/extension.ts` (register command)
- `vscode-extension/package.json` (commands, keybindings, configuration)

Do NOT modify store schema, CLI args, MCP tools, or any crate outside
`aether-dashboard` and `vscode-extension`.

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aether-dashboard -- -D warnings
cargo test -p aether-dashboard

cd vscode-extension
npm install
npm run build
```

Do NOT run `cargo test --workspace` — OOM risk.

All commands must pass before committing.

## COMMIT

```bash
git add -A
git commit -m "feat(vscode): prompt enhancer command with daemon HTTP API"
```

**PR title:** `feat(vscode): prompt enhancer command with daemon HTTP API`

**PR body:**
```
Stage CC.7 of the Claude Code Integration phase.

Adds in-editor prompt enhancement via VS Code command:
- New daemon HTTP endpoint: POST /api/enhance (on dashboard port 9730)
- VS Code command: AETHER: Enhance Prompt (Ctrl+Shift+E or Ctrl+Alt+E)
- Replaces selected text with AETHER-enriched prompt
- Falls back to CLI when daemon unavailable
- Works in VS Code, Cursor, Windsurf, and any VS Code fork
- Configurable token budget and LLM rewrite mode

Reuses CC.6 core enhancement logic via dashboard HTTP server.
```

## POST-COMMIT

```bash
git push origin feature/cc7-vscode-enhancer
# Create PR via GitHub web UI with title + body above
# After merge:
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/cc7-vscode-enhancer
git branch -D feature/cc7-vscode-enhancer
```
