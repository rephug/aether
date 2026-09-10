# Phase 9 — Stage 9.6: Integrated Agent Loop

**Status:** Spec complete, not implemented
**Decisions:** #122–#125 (+ per-workflow overrides folded into #125)
**Depends on:** Stage 9.2 (Configuration UI); benefits from Stage 10.3
(Agent Hooks) but degrades gracefully to the existing 40 MCP tools
**Estimated:** 3–4 Codex runs

## Purpose

Phase 9 as shipped is a dashboard viewer — the core intelligence workflows
(/next, /audit, /enrich) still require Claude Code and terminal
familiarity. Stage 9.6 makes the Tauri app a self-contained product: users
run audit, enrichment, and refactor workflows entirely from the GUI.

## Architecture

A built-in agent orchestrator inside the Tauri app with three execution
modes, selectable in settings:

1. **Claude Max subprocess mode** — spawns
   `claude -p --model <model> --allowedTools "mcp__aether*"`, pipes the
   workflow prompt on stdin, parses stdout for progress. $0 marginal cost
   on a Max subscription.
2. **Direct API mode** — Anthropic, OpenAI, Google, OpenRouter, DeepSeek,
   Ollama, or any OpenAI-compatible endpoint. Pay per token.
3. **Ollama local mode** — localhost, $0, fully offline.

Claude Code remains an optional power interface connecting to the app's
MCP server (HTTP/SSE, port ~9720) — both paths write to the same
SharedState stores and the dashboard reflects results from either.

## Locked decisions

### 122. Dual-mode operation
Built-in agent (calls LLM APIs directly with in-process tool execution)
runs simultaneously alongside the Claude Code bridge (MCP server exposed
for external connection). Both write to the same stores; the dashboard
shows activity from both.

### 123. Separate agent config
The `[agent]` section is independent from `[inference]`. Cheap model for
bulk SIR generation; smart model for agent orchestration.

### 124. Two API formats
Anthropic-native format when the endpoint is `anthropic.com`;
OpenAI-compatible format for everything else (OpenAI, OpenRouter,
DeepSeek, Ollama, etc.).

### 125. In-process tool execution + per-workflow model overrides
The built-in agent calls MCP tool handlers directly against `SharedState`
in Rust — no network hop, no serialization; the MCP transport is only for
external agents. Model selection is per workflow via config, mapping to
`claude -p --model <x>` in Max mode:

```toml
[agent]
mode = "claude_max"          # claude_max | api | ollama
model = "sonnet"             # default

[agent.workflows.audit]
model = "haiku"              # cheap triage

[agent.workflows.enrich]
model = "opus"               # best quality

[agent.workflows.next]
model = "haiku"              # just picking a target
```

## Feature surface

- Workflow templates: audit, enrich, next, refactor, custom prompt
- SSE-streamed activity log in the UI
- Cost tracking with monthly ceilings (API mode only; Max/Ollama are $0)
- Claude Code connection instructions panel with auto-populated
  `claude mcp add aether --url http://localhost:9720/mcp` command
- "Agent Integration" settings button invoking `run_init_agent` internally
  and displaying generated files with copy buttons (closes the
  init-agent-is-CLI-only gap)

## Constraint

SurrealKV holds an exclusive file lock — a separate `aetherd` process
cannot run via stdio while the Tauri app is open. HTTP/SSE MCP is the only
viable transport for coexistence with Claude Code.
