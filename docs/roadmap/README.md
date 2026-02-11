# AETHER Roadmap

This directory defines the implementation roadmap for AETHER using the existing workspace crates:
`aetherd`, `aether-core`, `aether-config`, `aether-parse`, `aether-sir`, `aether-store`, `aether-infer`, `aether-lsp`, and `aether-mcp`.

## Phase 1: Observer
- [Phase 1 Overview](./phase_1_observer.md)
- [Stage 1: Indexing](./phase_1_stage_1_indexing.md)
- [Stage 2: SIR Generation](./phase_1_stage_2_sir_generation.md)
- [Stage 3.1: Lexical Search](./phase_1_stage_3_1_search_lexical.md)
- [Stage 3.2: Robustness](./phase_1_stage_3_2_robustness.md)
- [Stage 3.3: SQLite SIR Source of Truth](./phase_1_stage_3_3_sir_sqlite_source_of_truth.md)
- [Stage 3.4: Semantic Search (Embeddings)](./phase_1_stage_3_4_semantic_search_embeddings.md)
- [Stage 3.5: CLI UX and Config](./phase_1_stage_3_5_cli_ux_and_config.md)
- [Stage 3.6: MCP and LSP UX](./phase_1_stage_3_6_mcp_and_lsp_ux.md)
- [Stage 3.7: VS Code Extension Polish](./phase_1_stage_3_7_vscode_extension_polish.md)
- [Stage 3.8: CI and Release Packaging](./phase_1_stage_3_8_ci_release_packaging.md)

## Phase 2: Historian
- [Phase 2 Overview](./phase_2_historian.md)
- [Stage 2.1: SIR Versioning](./phase_2_stage_2_1_sir_versioning.md)
- [Stage 2.2: Git Linkage](./phase_2_stage_2_2_git_linkage.md)
- [Stage 2.3: Why Queries](./phase_2_stage_2_3_why_queries.md)

## Phase 3: Ghost
- [Phase 3 Overview](./phase_3_ghost.md)
- [Stage 3.1: Host Verification](./phase_3_stage_3_1_host_verification.md)
- [Stage 3.2: Container Verification](./phase_3_stage_3_2_container_verification.md)
- [Stage 3.3: MicroVM (Optional)](./phase_3_stage_3_3_microvm_optional.md)

## Notes
- Every phase/stage file includes purpose, scope boundaries, testable pass criteria, and a copy/paste Codex prompt.
- Legacy drafts currently in this folder: `00_overview.md`, `promptlist.md`.
