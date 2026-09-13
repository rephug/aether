# AETHER Quickstart — From Zero to AI-Powered Coding

This guide takes you from a fresh project to a fully AETHER-integrated AI
coding workflow. By the end, your AI coding agent (Claude Code, Codex,
Cursor) will automatically understand your codebase's semantic intent
before writing code and verify its changes afterward.

**Time to first value:** ~15 minutes (index + init-agent).
**Time to full workflow:** ~1 hour (add a scan/enrichment pass).

## 1. Index your codebase

Zero-API-key path (Claude Code Max subscribers):

    aetherd --workspace . --index-once --full --inference-provider mock

This builds the symbol table and dependency graph with tree-sitter only.
Every symbol gets a `[MOCK]` placeholder SIR at confidence 0.1 — the
`/scan` step below replaces them with real annotations at $0 cost.

Gemini path (faster for 10K+ symbol codebases, ~$2):

    aetherd --workspace . --index-once   # with [inference] configured for Gemini

## 2. Wire up your AI agent

    aetherd --workspace . init-agent --platform claude   # or gemini | codex | cursor | all

This generates CLAUDE.md (behavioral guidance + required actions), a
skill file, the AETHER slash commands under `.claude/commands/`,
`scripts/scan_all.sh`, and a project-scoped `.mcp.json` that registers the
AETHER MCP server (the stdio `aether-mcp` binary; no daemon needs to be
running). To register it by hand instead:

    claude mcp add --transport stdio --scope project aether -- aether-mcp --workspace .

## 3. Get baseline coverage

    ./scripts/scan_all.sh        # parallel /scan across all crates, $0

## 4. Deepen quality where it matters

    /next          # health-guided advisor picks the highest-value target
    /enrich <crate>  # deep SIRs with reasoning traces (Max subscription, $0)

## 5. The daily loop

    /health-check            # 30-second morning check-in
    /explain-area <target>   # orient in unfamiliar code
    /pre-flight <task>       # briefing before you start
    ... implement ...
    /review                  # semantic review vs SIR intent before commit
    /impact <symbol>         # blast radius before risky changes

## Troubleshooting

- **"attempt to write a readonly database" / lock errors:** stop the
  daemon before CLI commands: `pkill -f aetherd && rm -f .aether/graph/LOCK`
- **Semantic search feels stale after bulk enrichment:** run
  `aetherd regenerate --embed-only` (until auto-refresh ships)
- Dashboard lives at http://localhost:9730 when the daemon is running.
