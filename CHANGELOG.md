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

- Python call edges follow package re-exports and `src/` layouts: `from pkg import f`,
  where `pkg/__init__.py` re-exports `f` from a submodule, now reaches the defining
  module's symbol, and tests outside `src/` reach `src.pkg…` symbols. Targets Python could
  bind more than one way stay unresolved. The first `aetherd batch run` afterwards
  regenerates the SIRs of Python symbols whose callers or callees newly resolve, because
  their neighbor context changes.
- `aether_sir_inject` refreshes the symbol's embedding after the write, so semantic
  search reflects injected SIRs immediately (#151).
- `aether_sir_inject` responses round confidence to four decimal places (#151).
- Ollama deep-pass runs keep the model's `<think>` block as the SIR's reasoning trace
  (#151).
