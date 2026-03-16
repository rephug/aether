# Codex Prompt — Phase Repo R.2 + R.3 + R.4: File Slicing, Presets, Multi-Format Output

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Never `cargo test --workspace` — always per-crate.

Read these files before writing any code:
- `docs/roadmap/phase_repo_the_prompter.md` (full spec — read R.2, R.3, R.4 sections)
- `crates/aetherd/src/sir_context.rs` (R.1 shared export engine — 3781 lines, already a god file. Add new code in separate files where possible)
- `crates/aetherd/src/cli.rs` (ContextArgs, Commands enum)
- `crates/aetherd/src/main.rs` (run_subcommand dispatch)
- `crates/aetherd/src/sir_agent_support.rs` (extract_symbol_source_text, load_fresh_symbol_source — reuse for slicing)
- `crates/aether-parse/src/parser.rs` (SymbolExtractor::extract_from_path — for reparsing to get symbol ranges)
- `crates/aether-core/src/lib.rs` (Symbol struct with SourceRange { start, end, start_byte, end_byte })
- `crates/aether-store/src/symbols.rs` (SymbolRecord — does NOT have source ranges, only signature_fingerprint)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add /home/rephu/aether-phase-repo-r2r3r4 -b feature/phase-repo-r2-r3-r4
cd /home/rephu/aether-phase-repo-r2r3r4
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any are false, STOP and report:

1. R.1 and 10.6 are merged. `sir_context.rs` has `pub enum ContextFormat { Markdown, Json }`, `pub(crate) fn render_export_document`, `pub(crate) fn parse_context_format`, `pub struct SourceBlock { language, content }`, `pub(crate) fn allocate_export_document`.
2. `ContextArgs` in `cli.rs` has: `targets`, `--symbol`, `--overview`, `--branch`, `--budget`, `--depth`, `--task`, `--include`, `--exclude`, `--format`, `--output`. No `--preset` or `--context-lines` yet.
3. **`symbols` SQLite table does NOT store source ranges.** SymbolRecord has: id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at. To get ranges, you must reparse the source file via `SymbolExtractor::extract_from_path(path, &source)` → `Vec<Symbol>` where `Symbol` has `range: SourceRange`.
4. `sir_agent_support.rs` has `extract_symbol_source_text(source, range)` which extracts a substring from source using `SourceRange` byte offsets — reuse this for slicing.
5. `SourceBlock` is currently a simple `{ language, content }` struct used for whole-file inclusion.
6. Schema version is **11** (from 10.6). No schema migration needed for R.2-R.4.
7. `sir_context.rs` is 3781 lines — already over the 1500-line god file threshold. New rendering code (XML, compact) and file slicing should go in **separate files** to avoid making this worse.

## IMPLEMENTATION

This prompt implements three independent features in one commit. They share no dependencies on each other but all extend the R.1 context engine.

---

### PART A: File Slicing (R.2)

#### A1: File slicer module

Create `crates/aetherd/src/context_slicer.rs`:

```rust
pub struct FileSlice {
    pub file_path: String,
    pub language: String,
    pub sections: Vec<SliceSection>,
    pub omitted_lines: usize,
    pub total_lines: usize,
}

pub struct SliceSection {
    pub symbol_name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source: String,
}
```

Implement `pub fn slice_file_for_context(workspace, file_path, target_symbol_ids, neighbor_symbol_ids, depth, context_lines) -> Result<FileSlice>`:

1. Read the source file from disk
2. Reparse via `SymbolExtractor::extract_from_path(path, &source)` to get `Vec<Symbol>` with `SourceRange`
3. Classify each parsed symbol:
   - **Primary target (depth 0):** full symbol body
   - **Direct neighbor (depth 1):** signature line + first doc comment line. If the symbol is ≤5 lines, include whole body.
   - **Depth 2+:** signature only (first line of the symbol range)
4. Sort all selected ranges by start line
5. **Merge adjacent:** if two ranges are within 5 lines of each other, merge into one continuous range
6. Add `context_lines` (default 3) before/after each range, clamped to file boundaries
7. Between non-adjacent ranges, insert elision marker: `// ... ({N} lines omitted) ...`
8. **Small file fallback:** files under 50 lines → return whole file content, skip slicing
9. Track `omitted_lines = total_lines - included_lines`

#### A2: Integrate with assembly engine

In `sir_context.rs`, modify the source layer preparation:
- When `LayerSelection.source` is true and the file has indexed symbols, call `slice_file_for_context` instead of reading the whole file
- The `SourceBlock.content` now contains the sliced output with elision markers
- If slicing fails (parse error), fall back to whole-file inclusion with a notice
- Add `tokens_saved` to `BudgetUsage` or a notice: "Source sliced: {N} of {M} lines included ({saved} tokens saved)"

#### A3: CLI flag

Add to `ContextArgs` in `cli.rs`:

```rust
#[arg(long, default_value_t = 3, help = "Context lines before/after each symbol slice")]
pub context_lines: usize,
```

Thread through to the slicer.

---

### PART B: Prompt Presets (R.3)

#### B1: Preset data model

Create `crates/aetherd/src/context_presets.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetConfig {
    pub preset: PresetMeta,
    pub context: PresetContextSettings,
    #[serde(default)]
    pub task_template: Option<PresetTaskTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetContextSettings {
    #[serde(default = "default_budget")] pub budget: usize,
    #[serde(default = "default_depth")] pub depth: u32,
    #[serde(default)] pub include: Vec<String>,
    #[serde(default)] pub exclude: Vec<String>,
    #[serde(default = "default_format")] pub format: String,
    #[serde(default = "default_context_lines")] pub context_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetTaskTemplate {
    pub template: String,
}
```

#### B2: Built-in presets (embedded in binary)

```rust
fn builtin_presets() -> Vec<PresetConfig> {
    vec![
        // "quick" — 8K, depth 1, sir + source
        // "review" — 32K, depth 2, sir + source + graph + coupling + health + tests
        // "deep" — 64K, depth 3, all layers
        // "overview" — 16K, depth 0, sir + health + drift
    ]
}
```

#### B3: Preset loading

```rust
pub fn load_preset(workspace: &Path, name: &str) -> Result<PresetConfig>
pub fn list_presets(workspace: &Path) -> Result<Vec<PresetConfig>>
```

1. Scan `.aether/presets/*.toml` for user presets
2. Merge with built-in presets (user overrides built-in with same name)
3. Return sorted by name

#### B4: CLI commands

Add to `Commands` enum:

```rust
/// Manage context presets
Preset(PresetArgs),
```

```rust
#[derive(Debug, Clone, Subcommand)]
pub enum PresetCommand {
    /// List available presets
    List,
    /// Show details of a preset
    Show(PresetShowArgs),
    /// Create a new user preset
    Create(PresetCreateArgs),
    /// Delete a user preset
    Delete(PresetDeleteArgs),
}
```

Add to `ContextArgs`:

```rust
#[arg(long, help = "Apply a named preset (CLI flags override preset values)")]
pub preset: Option<String>,
```

**Application order:** preset values first, then CLI flags override. If `--preset deep --budget 16000`, budget is 16000 (CLI wins).

#### B5: Task template substitution

When a preset has `task_template.template`, substitute `{target}` with the actual file/symbol targets joined by ", ". Use the result as the `--task` value if `--task` is not explicitly provided.

---

### PART C: XML + Compact Formats (R.4)

#### C1: Extend ContextFormat

In `sir_context.rs`, add variants:

```rust
pub enum ContextFormat {
    Markdown,
    Json,
    Xml,
    Compact,
}
```

Update `parse_context_format` to accept `"xml"` and `"compact"`.

#### C2: Renderers in separate files

Create `crates/aetherd/src/context_renderers.rs`:

```rust
pub fn render_xml(document: &ExportDocument) -> String { ... }
pub fn render_compact(document: &ExportDocument) -> String { ... }
```

**XML format:**
```xml
<aether_context workspace="..." generated_at="...">
  <overview symbols="..." sir_coverage="..." health="..." />
  <target kind="file" path="...">
    <symbol name="..." kind="..." staleness="...">
      <sir>
        <intent>...</intent>
        <side_effects>...</side_effects>
        <error_modes>...</error_modes>
        <dependencies>...</dependencies>
      </sir>
      <source language="..."><![CDATA[...]]></source>
    </symbol>
    ...
  </target>
  <graph>
    <neighbor name="..." kind="..." intent="..." />
  </graph>
  <coupling>
    <pair file_a="..." file_b="..." score="..." />
  </coupling>
  <tests>
    <guard name="..." description="..." />
  </tests>
  <budget used="..." max="..." />
</aether_context>
```

Use `<![CDATA[...]]>` for source code blocks and any content that might contain XML-special characters. Escape `]]>` sequences inside CDATA if they appear in source.

**Compact format:**
```
=== AETHER Context: {target} ===
Budget: {used}/{max} | Symbols: {count} | Health: {score}

[{SymbolName}] {Kind}
  Intent: {first sentence of intent}
  Deps: {dep1}, {dep2}, ...
  Callers: {caller1} ({file}), ...

[{SymbolName2}] {Kind}
  ...
```

One-symbol-per-block, minimal whitespace, no markdown formatting. Every line carries information.

#### C3: Wire into render_export_document

In `sir_context.rs`, update `render_export_document`:

```rust
pub(crate) fn render_export_document(document: &ExportDocument, format: ContextFormat) -> String {
    match format {
        ContextFormat::Markdown => render_export_markdown(document),
        ContextFormat::Json => serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".to_owned()),
        ContextFormat::Xml => context_renderers::render_xml(document),
        ContextFormat::Compact => context_renderers::render_compact(document),
    }
}
```

---

### Module registration

In `crates/aetherd/src/lib.rs`, add:

```rust
pub mod context_presets;
pub mod context_renderers;
pub mod context_slicer;
```

## SCOPE GUARD — Do NOT modify

- R.1's `ExportDocument` struct fields (only ADD to it if needed for slice metadata)
- Legacy `sir-context` behavior
- Task context engine (10.6)
- Existing batch/watcher/continuous behavior
- Existing dashboard pages

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

Verify CLI:
```bash
$CARGO_TARGET_DIR/debug/aetherd context --help
$CARGO_TARGET_DIR/debug/aetherd preset --help
$CARGO_TARGET_DIR/debug/aetherd preset list --help
$CARGO_TARGET_DIR/debug/aetherd preset show --help
```

### Validation criteria

**R.2 (Slicing):**
1. Single-symbol extraction returns correct source range
2. Two adjacent symbols within 5 lines merge into single range
3. Elision markers show correct omitted line counts
4. Files under 50 lines return whole file
5. `--context-lines` controls padding around slices
6. Token savings reported in output notices
7. Parse failure falls back to whole-file with notice

**R.3 (Presets):**
8. `preset list` shows 4 built-in presets (quick, review, deep, overview)
9. `context --preset deep` applies budget=64K, depth=3, all layers
10. CLI flags override preset values: `--preset deep --budget 16000` → budget 16000
11. User preset in `.aether/presets/` overrides built-in with same name
12. Task template `{target}` substitution works
13. Invalid TOML produces clear error message
14. `preset create` and `preset delete` manage `.aether/presets/` files

**R.4 (Formats):**
15. `--format xml` produces valid XML with `<aether_context>` root
16. Source code in XML uses `<![CDATA[...]]>` blocks
17. `--format compact` produces denser output than markdown for same content
18. All four formats include budget usage metadata
19. JSON output unchanged from R.1

**Cross-cutting:**
20. Legacy `sir-context` still works unchanged
21. Existing `context` command (file/symbol/overview/branch modes) still works
22. `cargo fmt --all --check`, `cargo clippy -p aetherd -- -D warnings`, `cargo test -p aetherd` pass

## COMMIT

```bash
git add -A
git commit -m "Phase Repo R.2+R.3+R.4: File slicing, presets, XML/compact formats

File slicing (R.2):
- Symbol-guided source slicing using tree-sitter parse spans
- Depth 0: full body, depth 1: signature + doc, depth 2+: signature only
- Adjacent symbol ranges within 5 lines merged automatically
- Elision markers with omitted line counts between non-adjacent ranges
- Small files under 50 lines return whole file
- --context-lines flag (default 3) controls padding
- Falls back to whole-file on parse failure with notice

Presets (R.3):
- TOML presets in .aether/presets/ with 4 built-in defaults
- quick (8K/depth 1), review (32K/depth 2), deep (64K/depth 3), overview (16K/depth 0)
- aetherd preset list/show/create/delete for management
- context --preset applies settings, CLI flags override
- Task template {target} variable substitution
- User presets override built-ins with same name

Output formats (R.4):
- XML format with <aether_context> root and CDATA source blocks
- Compact format with maximum density single-line-per-symbol layout
- Renderers in separate context_renderers.rs module
- All four formats (markdown, json, xml, compact) include budget metadata"
```

**PR title:** Phase Repo R.2+R.3+R.4: File slicing, presets, XML/compact formats
**PR body:** Three independent enhancements to the context export engine: symbol-guided file slicing for 50-80% source token reduction, TOML-based reusable presets with 4 built-in defaults, and XML + compact output formatters. New `--context-lines`, `--preset` flags on `context` command. New `preset` subcommand group. All rendering code in separate modules to avoid growing sir_context.rs further.

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase-repo-r2-r3-r4
```
