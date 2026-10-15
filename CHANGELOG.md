# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Go parsing: functions, methods (`pkg::Type::Method`), structs, interfaces and named types;
  call, import and type-reference edges; `func TestXxx(t *testing.T)` test intents.
- Java parsing: classes, interfaces, enums, records, methods and constructors
  (`Outer::Inner::method`); call, import, type-reference and `extends`/`implements`
  edges; JUnit `@Test` test intents (with `@DisplayName`).
- C# parsing: classes, structs, records, interfaces, enums, delegates, methods and
  constructors (`Outer::Inner::Method`, namespaces left out); call, `using`,
  type-reference and base-list edges; xUnit, NUnit and MSTest test intents.
- `inference.provider = "omp"`: run SIR generation through the Oh My Pi auth gateway
  (`omp auth-gateway serve`) so models are addressed as omp routes (`provider/model`)
  and billed on the omp credential (subscription logins included). The gateway token is
  read from `OMP_GATEWAY_TOKEN` or `~/.omp/auth-gateway.token`; `inference.thinking` is
  forwarded as `reasoning_effort`. Also accepted as `[inference.tiered].primary` and as
  `sir_quality.triage_provider` / `deep_provider`.
- `batch.provider = "auto"`: derive the batch provider from the omp route in
  `inference.model` so batch pricing (Anthropic Message Batches, OpenAI Batch, Gemini
  Batch Mode) is one switch away for providers that offer it, with an explicit error for
  routes that do not. Batch model fields accept omp routes and fall back to the bare
  model of `inference.model`.
- `aetherd init-agent --platform omp` writes `AGENTS.md` and a project-root `.mcp.json`
  for Oh My Pi projects.
- `aetherd omp up|down|status` and `[inference.omp] autostart` (default on): AETHER
  spawns `omp auth-broker serve` and `omp auth-gateway serve` itself when the gateway is
  unreachable, so an `omp` login is all that is needed before `aetherd index`.

- GitHub Release packaging now publishes `aetherd` and `aether-mcp` binaries for
  x64 and arm64 across Linux, macOS, and Windows targets.
- Zero-key onboarding (#152): `inference.provider = "mock"` writes `[MOCK]` placeholder
  SIRs from tree-sitter facts at confidence 0.1, and `init-agent --platform claude` now
  generates the `/scan` slash command, `scripts/scan_all.sh` and a project-scoped
  `.mcp.json`, so Claude Code sessions can replace the placeholders with scan-level SIRs
  without an inference API key.
- `aether_audit_candidates` takes `include_deep` (#151); by default it now leaves out
  symbols that already have a usable deep-pass SIR.

### Changed

- CI now runs explicit required jobs for `fmt`, `clippy`, and workspace tests.
- Release workflow now supports both semver tag pushes (`v*.*.*`) and manual
  dispatch with a required `tag` input.
- SIR and embedding writers coordinate through inject and per-symbol embedding locks,
  identity guards and source-hash binding (#152). Core schema 18 → 21; agent template
  schema 3 → 4.

### Fixed

- `aetherd --index-once` no longer serves stale SIR after edits made while nothing was
  watching. A symbol whose body changed since its SIR was written (its recorded
  `source_hash` no longer matches) is marked `stale`, including a SIR that
  reconciliation migrated onto a re-identified symbol; `--full` regenerates every stale
  leaf. Plain `--index-once` now reconciles and prunes like `--full`: symbols deleted
  from the workspace leave `symbols`, `sir` and their `.aether/sir/<id>.json` mirrors.
  File rollups of changed files are marked `stale` (or removed when no leaf SIR is
  left), and `--full` rebuilds stale and missing rollups. Pruning now runs before the
  graph sync, so stale ids are no longer written to the graph first.
- The watching daemon catches edits made while it was stopped. Its startup syncs the
  store like `--index-once` does: removed symbols are pruned, and changed ones are
  marked stale. It then queues stale leaves alongside SIR-less ones, and rebuilds
  outdated file rollups once the queue drains. `aetherd batch extract` and the
  continuous monitor's structural refresh sync the same way.
- `inference.provider = "openai_compat"` now sends `[inference] thinking` as
  `reasoning_effort`, as `omp` already did. `[inference] timeout_secs` now lifts the
  request timeout of `openai_compat` and `omp` as well as `qwen3_local`, so a model that
  thinks for more than 120 seconds on one SIR is no longer cut off by the HTTP client.
- `aether_sir_inject` refreshes the symbol's embedding after the write, so semantic
  search reflects injected SIRs immediately (#151).
- `aether_sir_inject` responses round confidence to four decimal places (#151).
- Ollama deep-pass runs keep the model's `<think>` block as the SIR's reasoning trace
  (#151).
