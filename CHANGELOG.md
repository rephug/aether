# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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

### Changed

- CI now runs explicit required jobs for `fmt`, `clippy`, and workspace tests.
- Release workflow now supports both semver tag pushes (`v*.*.*`) and manual
  dispatch with a required `tag` input.
