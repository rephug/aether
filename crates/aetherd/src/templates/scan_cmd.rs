use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanCommandTemplate;

impl ScanCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
description: Fast baseline SIR coverage for a crate — replaces [MOCK] and low-confidence SIRs, coverage over depth
---

# /scan — zero-key baseline coverage

Usage: `/scan <crate> [batch-size] [scopes=<paths>]`

Purpose: give every symbol in `<crate>` that only has a `[MOCK]` placeholder or a
low-confidence (< 0.2) SIR a real, scan-level SIR. This is about COVERAGE, not
perfection: deeper enrichment comes later. `batch-size` (default 100) caps how many
symbols this session processes before stopping. `scopes=` (what `scripts/scan_all.sh`
passes) is a comma-separated list of project-relative paths, each an include or, with a
leading `-`, an exclude: use exactly those and skip step 1; `<crate>` is then only a
label (it may read `dir:web` or `shared` for a synthetic unit).

## Procedure

1. Resolve the crate's directories (skip this step when `scopes=` was given: its
   includes are the `<dir>` set and its `-` entries the exclusions). All paths below are relative to THIS project's
   root (the directory holding `.aether/`, the same root the indexed `file_path`s use),
   never to an enclosing Cargo workspace root when the two differ. In a Cargo project
   run `cargo metadata --no-deps --format-version 1` and take the directory of the
   `manifest_path` for the package named `<crate>`, made relative to the project root
   (for example `crates/<crate>` or `packages/<crate>`); ignore packages outside the
   project. Call it `<dir>`. Without Cargo (TypeScript, Python, ...), or when `<crate>`
   names no package, `<crate>` is a directory (or a single file) relative to the project
   root. A package also owns every target whose `src_path` lies outside its manifest
   directory (for example `[lib] path = "../../shared/foo.rs"`): when it is the only
   package with a target in that directory, add the directory (`shared`, so sibling
   modules such as `mod util;` → `shared/util.rs` are covered); when several packages
   target the same directory, add only that file (exact `file_path`) and its `<stem>/`
   module directory (`shared/foo`), so `shared/bar.rs` of another package stays out and
   the directory's remainder is scanned as its own unit by `scripts/scan_all.sh`; when
   several packages declare the very same target file, only the first package name in
   sorted order owns it. Paths never start with `./`. If the manifest sits at the
   project root, the package owns only its own targets: use the top-level directory of
   each target's `src_path` (typically `src`, `tests`, `benches`, `examples`) as the
   `<dir>` set, and match a target file stored at the root itself (for example
   `path = "lib.rs"`) by exact `file_path`. Never treat a root package as "the whole
   workspace": other members live beside it. If another package's directory lies
   inside `<dir>` (`crates/parent` and `crates/parent/child`), exclude it
   (`AND NOT s.file_path LIKE '<nested>/%' ESCAPE '\'`) so no two crates scan the
   same symbols. If `cargo metadata` fails, fall back to the directory rule above.
2. Build the target list from the low-confidence set, not from a ranked window.
   Query `.aether/meta.sqlite` directly, so symbols that were already scanned can never
   crowd the remaining placeholders out of a truncated result:
   `SELECT s.id, s.qualified_name, s.file_path FROM symbols s JOIN sir ON sir.id = s.id
   WHERE s.file_path LIKE '<dir>/%' ESCAPE '\'
     AND (json_extract(sir.sir_json, '$.confidence') < 0.2 OR sir.sir_json LIKE '%"intent":"[MOCK]%'
          OR sir.sir_status = 'rollup_failed')
   ORDER BY s.file_path, s.qualified_name`
   (for several `<dir>`s, OR one `LIKE` clause per directory; a root-level file is
   matched with `s.file_path = '<file>'`, so use `(s.file_path = '<p>' OR s.file_path
   LIKE '<p>/%' ESCAPE '\')` per include when unsure; every exclusion becomes
   `AND NOT (<same predicate>)`; escape `_`, `%` and `\` in paths with a
   backslash so they match literally). Run it with the `sqlite3` CLI, or, when that is
   not installed, with Python's built-in module:
   `python3 -c "import sqlite3; [print(*r) for r in sqlite3.connect('.aether/meta.sqlite').execute(\"<query>\")]"`.
   Only if neither is available fall back to `aether_audit_candidates`: its `top_n` is
   capped at 200 and it ranks by a composite score in which confidence is only one
   factor, so it is reliable only for crates with fewer than 200 symbols; call it with
   `top_n: 200`, keep only candidates under `<dir>` whose `current_confidence` is below
   0.2 or whose SIR intent starts with `[MOCK]`, and note that its `crate_filter`
   assumes a `crates/<crate>/` layout, so filter on `file_path` yourself. Never pass
   `batch-size` as `top_n`.
3. Take the first `batch-size` targets and group them by source file.
4. Work in batches of 10 symbols per reasoning turn: read each source file ONCE,
   produce the SIRs for all of its symbols together, then fire every
   `aether_sir_inject` call for the batch (intent, behavior, inputs, outputs,
   side_effects, dependencies, error_modes, complexity, confidence, and
   `generation_pass: "scan"`, `provider: "claude-code"`, `model: "<your model>"`).
5. Target confidence 0.7–0.8 for scan-level SIRs. Use `force: true` only when replacing a
   `[MOCK]` placeholder that somehow carries a higher confidence. If an inject call
   returns an error saying the SIR was written but the file rollup could not be rebuilt,
   rerun that same call once: the symbol is marked `rollup_failed`, the confidence guard
   is lifted for it, and the target query above keeps selecting it until the rerun succeeds.
6. Stop after `batch-size` symbols and print how many targets remain (rerun `/scan` or let
   `scripts/scan_all.sh` loop).

## Quality guidelines

1. Speed over depth. This is about COVERAGE, not perfection. A 0.75
   confidence SIR that correctly describes the function is far better
   than a [MOCK] placeholder. /enrich will improve it later.
2. Don't skip symbols. Every symbol deserves at least a basic SIR.
   Even trivial getters get a scan-level annotation.
3. Don't call aether_sir_context. That's the expensive cross-symbol
   lookup. Save it for /enrich. Just read the source file.
4. Don't write reasoning traces. They're valuable but eat context.
   Save them for /enrich deep passes.
5. DO flag anything suspicious. If you spot a potential bug while
   scanning, note it in error_modes but don't deep-dive.
6. Batch aggressively. Read 10 source files at once, produce all 10
   SIRs in a single reasoning step, then fire off all 10 inject
   calls. Per-turn overhead is the bottleneck — minimize turns, not
   tokens.
7. Group by file. When multiple symbols share a source file, read the
   file once and analyze all its symbols together. This is the single
   biggest throughput win.
"#
        .to_owned()
    }
}
