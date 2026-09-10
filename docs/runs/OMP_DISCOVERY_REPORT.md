# OMP × AETHER Discovery Report

Date: 2026-09-10. Read-only investigation. Nothing in either repo was modified except this file.

Environment caveat, stated up front: this run happened in a remote Claude Code container, not on the
dev box. The paths in the prompt (`/home/rephu/projects/...`) map here to `/home/user/aether` and
`/home/user/gnaughtybytegnature-omp`. omp was **not** installed in the container and **no provider
credentials exist here**, so I installed `@oh-my-pi/pi-coding-agent@18.0.10` (the exact version the
add-on pins) from npm into a scratch directory and ran every probe against it. Every probe that needed
a logged-in provider therefore stops at omp's "No API key found" boundary. That boundary is still
informative (it is the same code path a quota/auth failure takes), but section E cannot contain a real
model response or a real 429 sample. The source reading in B, C, D is against the installed 18.0.10
TypeScript (the npm package ships `src/`), so it is exact.

Two inputs named in the prompt do not exist in the AETHER repo as cloned: there is no
`docs/roadmap/phase_10_stage_10_1b_subprocess_cli_providers.md` (no file matches `10_1b`/`10.1b`
anywhere in the tree or git history), and there are no `scripts/scan_all.sh` / `scripts/enrich_all.sh`
(the only script is `scripts/gemini_batch_submit.sh`). Decision #127 is also not present in any
`docs/DECISIONS*.md` or `docs/roadmap/DECISIONS_v*.md` (the numbering there stops well short of 127).
I proceeded from the prompt's description of what those documents say.

## Executive summary (10 lines)

1. The add-on repo is a genuine omp **extension** (`package.json` → `"omp": {"extensions": ["./src/index.ts"]}`), pinned to `@oh-my-pi/pi-coding-agent` 18.0.10; it already contains a working `SdkAdapter` (in-process `createAgentSession`) and a `SubprocessAdapter` over `omp -p --mode json`, both directly reusable as prior art for AETHER's `OmpProvider`.
2. omp `--mode rpc` is a clean NDJSON protocol with typed request/response frames (`RpcCommand`/`RpcResponse` in `modes/rpc/rpc-types.ts`); final assistant text arrives in the `message_end`/`agent_end` session events and on demand via `get_last_assistant_text`.
3. `--tools` accepts MCP tools by their minted name `mcp__<server>_<tool>`; with a server named `aether`, `aether_status` becomes **`mcp__aether_status`**, and `--tools read,grep,glob,mcp__aether_status` was verified live to pin exactly that set (plus a force-added `write`).
4. omp loads project MCP servers from **`.mcp.json` / `mcp.json` at the project root** (setting `mcp.enableProjectConfig`, default true) — verified live with a fake stdio MCP server; the client handshake and `tools/list` were observed in the server log.
5. omp does **not** read a root-level `CLAUDE.md`; it reads `.claude/CLAUDE.md` and `~/.claude/CLAUDE.md`. AETHER's root `AGENTS.md` **is** injected as `<repo-rules>`; root `GEMINI.md`, `.cursorrules` and `.cursor/rules/*.mdc` did not appear in the system prompt.
6. `.claude/commands/*.md` **are** imported as slash commands (a probe `.claude/commands/audit.md` produced `/audit` with source `file`); `.claude/skills/*/SKILL.md` become `/skill:<name>` commands.
7. Cross-provider fallback (`retry.fallbackChains`) is **empty by default** (`{}`), so nothing falls back behind AETHER's back unless the user's global `~/.omp/agent/config.yml` configures it; a per-invocation `--config overlay.yml` with `retry: {fallbackChains: {}, modelFallback: false, usageAwareFallback: false}` overrides global and project settings without touching them.
8. Quota handling is rich and structured: `AssistantMessage.errorStatus` (HTTP), `errorId` (bit-flag classifier incl. `UsageLimit`), `errorMessage` with an appended `retry-after-ms=<n>` hint, `auto_retry_start{delayMs}` frames, and a fail-fast when the provider's wait exceeds `retry.maxDelayMs` (default 5 min) — enough to key pause-and-resume off real signals in rpc/json mode.
9. Same-provider **OAuth account round-robin** and per-credential block windows are built in and persisted in `~/.omp/agent/agent.db` (`auth_credential_blocks`), shared across processes; nothing in the code prevents 4–8 parallel rpc processes, but they share one sqlite auth store with WAL + busy_timeout.
10. Two gotchas that will bite a Rust spawner: (a) `omp -p` **blocks forever waiting for stdin EOF** when stdin is a pipe, so AETHER must close stdin or feed the prompt through it; (b) in `--mode json`, a provider error before the agent loop exits 1 with a noisy bun stack on stderr, while a mid-turn error surfaces only as `stopReason:"error"` inside frames with exit 0.

---
## A. What the add-on repo is

### A.1 Characterization

It is an **omp extension** (a TypeScript module omp loads in-process), packaged as an npm-style
project named `boonton`, plus a role/route config bundle and a test suite. Evidence:

- `package.json` (repo root): `"name": "boonton"`, `"description": "GnaughtyByteGnature OMP (engine: Boonton) — multi-model role-based orchestration for Oh My Pi"`, and the extension declaration
  ```json
  "omp": { "extensions": ["./src/index.ts"] },
  "peerDependencies": { "@oh-my-pi/pi-coding-agent": "18.0.10" },
  "devDependencies": { "@oh-my-pi/pi-coding-agent": "18.0.10", ... }
  ```
- `src/index.ts`: `export default function boonton(pi: ExtensionAPI): void { ... pi.on("session_start", ...) ... registerStatusCommand(pi, ...) ... }` — the classic omp extension entry point registering `/team-*` slash commands (status, opinion, fuse-plan, validate, build, explore, deliver, doctor, tasks, resolve, pause, away, brief, checkpoint, cancel, board).
- It is **not a fork** of omp (no omp sources vendored; `bun.lock` resolves `@oh-my-pi/*` from npm) and **not a wrapper script** (no shell launcher; `scripts/` holds smoke/spike/doctor TypeScript, not an `omp` wrapper).
- It contains its own orchestration engine (`src/orchestration/`, `src/lifecycle/`, `src/persistence/` with a SQLite run DB, `src/gates/` with a bubblewrap sandbox, `src/server/` control server + UI). Roles are prompt files under `skills/*.md` (architect, developer, explorer, final-judge, fusion-judge, opinion, plan-reviewer, qa-reviewer, triage, validator-gate).

### A.2 How it launches omp, selects models/roles, custom providers

It launches model work through **two adapters** selected by `adapter:` in config (`src/config/config.ts`: `adapter: "sdk" | "subprocess" | "fake"`, default `"sdk"` in `src/config/defaults.ts`):

- **`sdk` (primary)** — `src/adapter/sdk-adapter.ts`: in-process `createAgentSession({...})` from `@oh-my-pi/pi-coding-agent`, reusing the host session's `modelRegistry`/`authStorage`. Key options it passes (verbatim from the file):
  ```ts
  const { session } = await createAgentSession({
    authStorage, modelRegistry: this.deps.modelRegistry, model,
    thinkingLevel: req.route.thinking ?? "medium",
    settings: Settings.isolated({ "compaction.enabled": true, "retry.enabled": true,
      "autolearn.enabled": false, "memory.backend": "off", "generate_image.enabled": false,
      "speechgen.enabled": false, "astGrep.enabled": false, "astEdit.enabled": false }),
    sessionManager: SessionManager.inMemory(req.cwd),
    systemPrompt: [req.systemPrompt], toolNames: [...req.tools],
    restrictToolNames: !needsHook, ...(needsHook ? { preloadedCustomToolPaths: [] } : {}),
    enableMCP: false, enableLsp: false, spawns: "", agentId: req.labels.workerId,
    disableExtensionDiscovery: true, extensions: guardExtension ? [guardExtension] : undefined,
    cwd: req.cwd,
    ...(req.outputSchema ? { outputSchema: req.outputSchema, outputSchemaMode: "strict", requireYieldTool: true } : {}),
  } as never);
  ```
- **`subprocess` (debug fallback)** — `src/adapter/subprocess-adapter.ts`, one `omp` process per turn:
  ```ts
  export function buildSubprocessArgs(req: SpawnRequest): string[] {
    const args = ["-p", "--mode", "json", "--model", `${req.route.provider}/${req.route.id}`,
                  "--no-extensions", "--no-session"];
    if (req.systemPrompt) args.push("--append-system-prompt", req.systemPrompt);
    if (req.tools.length) args.push("--tools", req.tools.join(","));
    return args;
  }
  ```
  spawned as `Bun.spawn(["setsid", "omp", ...args], { cwd, stdin: "pipe", stdout: "pipe", stderr: "pipe" })`, prompt written to stdin then `stdin.end()`, NDJSON parsed from stdout with the last assistant text winning. (That `stdin.end()` is exactly what avoids the hang documented in B.1/E.2.)

Model selection is **role → route list**, not omp roles. `src/config/defaults.ts`:
```ts
roles: {
  architect:    { routes: [{ model: "anthropic/claude-fable-5", thinking: "high" }] },
  planReviewer: { routes: [{ model: "openai-codex/gpt-5.6-sol", thinking: "high" }] },
  fusionJudge:  { routes: [], inherit: "architect" },
  developer:    { routes: [{ model: "openai-codex/gpt-5.6-luna", thinking: "medium" }] },
  validator:    { routes: [{ model: "opencode-go/deepseek-v4-flash", thinking: "high" }] },
  qaReviewer:   { routes: [{ model: "openai-codex/gpt-5.6-sol", thinking: "high" },
                           { model: "opencode-go/deepseek-v4-flash", thinking: "high" }] },
  triage:       { routes: [{ model: "openai-codex/gpt-5.6-sol", thinking: "high" }] },
  explorer:     { routes: [], inherit: "architect" },
  opinion:      { routes: [{ model: "anthropic/claude-fable-5" }, { model: "openai-codex/gpt-5.6-sol" },
                           { model: "opencode-go/deepseek-v4-flash" }] },
},
```
A `provider/model` string is resolved through omp's registry (`src/adapter/registry-resolver.ts`: split on the **first** `/`, then `registry.hasConcreteAuth(provider)` fail-closed, then `registry.find(provider, id)`). Multiple routes on one role are **fan-out in parallel** (`src/orchestration/steps/final-qa.ts:68` maps over routes), **not** a fallback chain. Tool sets per role are fixed in `src/roles/roles.ts` (`READ_ONLY_TOOLS = ["read","grep","glob"]`; developer adds `edit,write,bash`; validator adds `write,bash`).

Custom providers: the repo defines **none**. There is no `models.yml`, `models.json` or provider definition in-repo; `docs/ENVIRONMENT.md` records that on the dev box the resolvable providers were `openai-codex, opencode-go, nanogpt, openrouter, zai, ollama` and, notably, **`anthropic` had no direct credential** ("Claude models must be routed through `nanogpt/` or `openrouter/`"). `.omp/boonton.yml` deliberately leaves z.ai undeclared because "this build has no provider id for it".

### A.3 Auth

- **Where credentials live**: nowhere in the repo. omp keeps them in the SQLite auth store `~/.omp/agent/agent.db` (tables observed live in this container after first run: `auth_credentials`, `auth_credential_blocks`, `auth_credential_refresh_leases`, `auth_credential_block_mirror_guard`, `auth_change_revision`, `auth_schema_version`, plus non-auth tables `cache, client_usage, clients, command_usage, meta, model_perf, model_usage, settings, usage_history`). omp's own error text confirms the path: `Use /login, set an API key environment variable, or create /root/.omp/agent/agent.db`. Custom models would go in `~/.omp/agent/models.yml` (also named in omp's error output). With `--profile <name>` / `OMP_PROFILE` the whole tree moves to `~/.omp/profiles/<name>/...` (pi-utils `dirs.ts:116`), and `PI_CODING_AGENT_DIR` relocates the agent dir wholesale.
- **How omp finds them**: `discoverAuthStorage(agentDir = getAgentDir())` in `pi-coding-agent/src/sdk.ts:728` returns an `AuthStorage` backed by `pi-ai/src/auth/sqlite-credential-store.ts`; `ModelRegistry` wraps it. The add-on uses the **registry's own** `authStorage` instance rather than calling `discoverAuthStorage()` again (`sdk-adapter.ts` comment: "createAgentSession rejects an authStorage that is not the registry's own instance"), falling back to `discoverAuthStorage()` only when no registry is supplied. `scripts/spike-auth.ts` is the probe that verified `hasProvider / hasConfiguredAuth / hasConcreteAuth`.
- **Does the add-on add providers or logins?** No. It only *observes* credential provenance: `src/environment/host.ts:157` `observeAuth()` reads `registry.getCredentialOrigin(provider).kind` (`CredentialOriginKind = "runtime" | "config" | "oauth" | "api_key" | "env" | "fallback"` in pi-ai `auth-storage.ts:129`) and `hasConcreteAuth`, and `src/environment/provider-policy.ts` vetoes a `billing: subscription` declaration when the origin is `api_key`/`env`/`config`. It never reads credential material (the file says so explicitly: "Nothing here ever reads credential material").

### A.4 Config surface

The add-on's only config file is `.omp/boonton.yml` (`.omp/fusion-team/` and `.omp/boonton/` are gitignored runtime dirs). It contains **no omp settings** — no `modelRoles`, `enabledModels`, `disabledProviders`, or `retry.fallbackChains`. The full file is policy for the add-on's own dogfood runs; the operative (non-comment) content is:

```yaml
dogfood:
  repository: rephug/gnaughtybytegnature-omp
  baseBranch: master
  forgeHost: github.com
  providers:
    openai-codex: { billing: subscription, overage: block, meteredFallback: false }
    opencode-go:  { billing: subscription, overage: block, meteredFallback: false }
    anthropic:    { billing: subscription, overage: block, meteredFallback: false }
    nanogpt:      { billing: metered,      overage: block, meteredFallback: false }
    ollama:       { billing: local,        overage: block, meteredFallback: false }
  approveMeteredSpend: []
  publish: off
```

**Fallback flag (relevant to AETHER decision #127):**

- The add-on itself has a *policy* field `meteredFallback: false` on every provider and an empty `approveMeteredSpend: []`, i.e. it is designed to **refuse** cross-provider/metered fallback. But this is the add-on's own gate, enforced by `/team-doctor --dogfood`; it does not write any omp setting.
- omp's own cross-provider fallback lives in `retry.fallbackChains` (schema below in C.3). In this container its effective value is `{}` (`omp config get retry.fallbackChains` → `{}`), which is the schema default. **Your global `~/.omp/agent/config.yml` on the dev box was not available to this session**, so whether it sets `retry.fallbackChains`, `modelRoles`, `enabledModels` or `disabledProviders` is unknowable from here — see Open Questions. The mitigation in F.4 works regardless of what the global file says.
- omp's *same-provider* OAuth account rotation (C.2) is always on and is not governed by `fallbackChains`.

## B. omp non-interactive surfaces

All findings are from the installed `@oh-my-pi/pi-coding-agent@18.0.10` (`omp --version` → `omp/18.0.10`),
source under `node_modules/@oh-my-pi/pi-coding-agent/src/`. Note: 18.0.10 requires Bun ≥ 1.3.14
(`engines.bun`); the container's Bun 1.3.11 failed to parse `dist/cli.js` (`using` declarations), so I
installed Bun 1.4.2 via npm. AETHER's spawner should check `bun --version` in its preflight.

### B.1 `omp -p` (print mode)

Exact flags, from `omp --help` (full output captured; the relevant subset):

```
  -p, --print                         Non-interactive mode: process prompt and exit
      --mode=<value>                  Output mode: text (default), json, rpc, or rpc-ui
      --model=<value>                 Model to use (fuzzy match: "opus", "gpt-5.2", or "openai/gpt-5.2")
      --smol/--slow/--plan=<value>    role models (or PI_SMOL_MODEL / PI_SLOW_MODEL / PI_PLAN_MODEL)
      --provider=<value>              Provider to use (legacy; prefer --model)
      --thinking=<value>              off, minimal, low, medium, high, xhigh, max, auto
      --system-prompt=<value>         System prompt (default: coding assistant prompt)
      --append-system-prompt=<value>  Append text or file contents to the system prompt
      --tools=<value>                 Comma-separated list of tools to enable (default: all)
      --no-tools / --no-lsp / --no-pty / --no-extensions / --no-skills / --no-rules / --no-session
      --config=<value>                Load an extra config.yml-style overlay for this run (repeatable)
      --profile=<value>               Use an isolated profile for auth, sessions, settings, and caches
      --cwd=<value>                   Directory to start in (overrides the launch cwd)
      --session-dir=<value>           Directory for session storage and lookup
      --max-time=<value>              Stop the session after this duration (e.g., 600, 10m, 1h)
      --auto-approve                  Auto-approve all tool calls
      --approval-mode=<value>         Override tools.approvalMode (always-ask|write|yolo)
      --print-thoughts                Include thinking blocks in print mode text output
      --no-title                      Disable title auto-generation
```

**Does `--model` accept `provider/model`?** Yes. `config/model-resolver.ts:206-230` `parseModelString()` splits on the first `/`; a trailing `:<level>` is a thinking suffix (`"claude-sonnet-4-6:high"`), and `@upstream` routing is also supported. Without a slash it fuzzy-matches (`matchModel` order: exact `provider/id` → exact bare id → retired alias → provider-scoped fuzzy → substring). For AETHER, always pass the full `provider/id[:level]` form so fuzzy matching can never pick a different model.

**Structured output flag:** `--mode json` emits one JSON object per line: a session header `{"type":"session","version":3,"id":...,"cwd":...}` first, then every `AgentSessionEvent` (B.2) shaped by `printableEvent()` (`modes/print-mode.ts`): `message_update` frames are reduced to the delta only, `providerPayload` stripped. There is no separate `--json` flag; `--mode json` is it.

**stdout vs stderr** (`print-mode.ts` + `main.ts:122`):
- text mode: stdout = final assistant text only (plus thinking with `--print-thoughts`); stderr = `Working...` indicator, notices, and on error the sanitized `errorMessage`.
- json mode: stdout = NDJSON events only; anything informational is routed to stderr (`(parsedArgs.mode === "json" ? process.stderr : process.stdout).write(text)`).

**Exit codes** (from source + live probes in E.2):
| Situation | Exit | Where |
|---|---|---|
| success | 0 | `main.ts:2101 postmortem.quit(0)` |
| text mode, final assistant `stopReason` is `error`/`aborted` | 1 | `print-mode.ts` "This branch hard-exits" → `process.exit(1)` with `errorMessage` on stderr |
| json mode, mid-turn provider error (429, 5xx after retries) | **0** | the `if (mode === "text")` guard means json mode never exits 1 for a turn error; the error is only in the `message_end`/`agent_end` frames (`stopReason:"error"`, `errorMessage`, `errorStatus`) |
| provider/auth failure **before** the agent loop (no credential) | 1 | thrown `Error("No API key found for <provider>...")` — uncaught, Bun prints a stack + `error:` line to stderr (observed, both modes) |
| model not found | 1 | `Model "x/y" not found` + hint on stderr, ~2.3 s |
| unknown flag | 2 | `reportUnrecognizedFlags` → `process.exit(2)` ("command line usage error") |
| `-p` with stdin a pipe and no EOF | **hangs** | main reads piped stdin as the prompt: `Reading prompt from piped stdin (waiting for EOF; ctrl+c to abort)…` — observed 120 s timeouts |

There is **no distinct exit code** for quota vs auth vs model error; all are 1. Discrimination must come from the json frames (B.2, C).

### B.2 `omp --mode rpc --no-session` frame schema

Verbatim from `src/modes/rpc/rpc-types.ts` (18.0.10). Commands go in on stdin, one JSON per line; responses and events come out on stdout.

Request frames (the subset AETHER needs, verbatim):
```ts
export type RpcCommand =
	// Protocol
	| { id?: string; type: "negotiate_protocol"; protocolVersion: number }
	// Prompting
	| { id?: string; type: "prompt"; message: string; images?: ImageContent[]; streamingBehavior?: "steer" | "followUp" }
	| { id?: string; type: "steer"; message: string; images?: ImageContent[] }
	| { id?: string; type: "follow_up"; message: string; images?: ImageContent[] }
	| { id?: string; type: "abort" }
	| { id?: string; type: "abort_and_prompt"; message: string; images?: ImageContent[] }
	| { id?: string; type: "new_session"; parentSession?: string }
	// State
	| { id?: string; type: "get_state" }
	| { id?: string; type: "get_available_commands" }
	| { id?: string; type: "set_host_tools"; tools: RpcHostToolDefinition[] }
	| { id?: string; type: "set_host_uri_schemes"; schemes: RpcHostUriSchemeDefinition[] }
	// Model
	| { id?: string; type: "set_model"; provider: string; modelId: string }
	| { id?: string; type: "cycle_model" }
	| { id?: string; type: "get_available_models" }
	// Thinking
	| { id?: string; type: "set_thinking_level"; level: ThinkingLevel }
	// Retry
	| { id?: string; type: "set_auto_retry"; enabled: boolean }
	| { id?: string; type: "abort_retry" }
	// Session
	| { id?: string; type: "get_session_stats" }
	| { id?: string; type: "get_last_assistant_text" }
	| { id?: string; type: "get_messages" }
	// Login
	| { id?: string; type: "get_login_providers" }
	| { id?: string; type: "login"; providerId: string };
```
(Also present, omitted here: `set_fast_mode`, `set_todos`, `set_subagent_subscription`, `get_subagents`, `get_subagent_messages`, `cycle_thinking_level`, `set_steering_mode`, `set_follow_up_mode`, `set_interrupt_mode`, `compact`, `set_auto_compaction`, `bash`, `abort_bash`, `export_html`, `switch_session`, `branch`, `get_branch_messages`, `set_session_name`, `handoff`, `get_messages_page`.)

Handshake frame, verbatim:
```ts
export interface RpcReadyFrame {
	type: "ready";
	protocolVersion: 1;
	supportedProtocolVersions: [1, 2];
	maxFrameBytes: number;
	maxReassembledFrameBytes: number;
}
```
(`rpc-frame.ts`: `MAX_RPC_FRAME_BYTES = 1024 * 1024`, `MAX_RPC_REASSEMBLED_BYTES = 64 * 1024 * 1024`; frames above 1 MiB are split into `{"type":"rpc_chunk","chunkId","index","count","byteLength","data"}` under protocol v2.)

Response frames, verbatim (relevant members):
```ts
export type RpcResponse =
	| { id?: string; type: "response"; command: "negotiate_protocol"; success: true; data: { protocolVersion: 2 } }
	| { id?: string; type: "response"; command: "prompt"; success: true; data?: { agentInvoked: boolean } }
	| { id?: string; type: "response"; command: "abort"; success: true }
	| { id?: string; type: "response"; command: "get_state"; success: true; data: RpcSessionState }
	| { id?: string; type: "response"; command: "set_model"; success: true; data: Model }
	| { id?: string; type: "response"; command: "get_available_models"; success: true; data: { models: Model[] } }
	| { id?: string; type: "response"; command: "set_thinking_level"; success: true }
	| { id?: string; type: "response"; command: "get_session_stats"; success: true; data: SessionStats }
	| { id?: string; type: "response"; command: "get_last_assistant_text"; success: true; data: { text: string | null } }
	| { id?: string; type: "response"; command: "get_messages"; success: true; data: { messages: AgentMessage[] } }
	| { id?: string; type: "response"; command: "get_login_providers"; success: true;
	    data: { providers: Array<{ id: string; name: string; available: boolean; authenticated: boolean }> } }
	// Error response (any command can fail); `code` is an optional machine-readable reason.
	| { id?: string; type: "response"; command: string; success: false; error: string; code?: string };
```

Session state returned by `get_state`, verbatim:
```ts
export interface RpcSessionState {
	model?: Model;
	thinkingLevel: ThinkingLevel | undefined;
	isStreaming: boolean;
	isCompacting: boolean;
	steeringMode: "all" | "one-at-a-time";
	followUpMode: "all" | "one-at-a-time";
	interruptMode: "immediate" | "wait";
	sessionFile?: string;
	sessionId: string;
	sessionName?: string;
	autoCompactionEnabled: boolean;
	fastModeEnabled: boolean;
	fastModeActive: boolean;
	tokensPerSecond: number | null;
	messageCount: number;
	queuedMessageCount: number;
	todoPhases: TodoPhase[];
	/** For session dump / export (plain-text parity with /dump). */
	systemPrompt?: string[];
	dumpTools?: Array<{ name: string; description: string; parameters: unknown; examples?: readonly ToolExample[] }>;
	/** Current context window usage. */
	contextUsage?: ContextUsage;
}
```

Event frames: `export type RpcSessionEventFrame = AgentSessionEvent | RpcSubagentFrame;` — every session event is written to stdout as-is (`rpc-mode.ts:978 session.subscribe(event => { output(event); })`). `AgentSessionEvent` (`session/agent-session-events.ts`, verbatim):
```ts
export type AgentSessionEvent =
	| Exclude<AgentEvent, { type: "agent_end" }>
	| (Extract<AgentEvent, { type: "agent_end" }> & {
			/** False when an async delivery will resume the session before its true final settle. */
			isTerminal?: boolean;
	  })
	| { type: "auto_compaction_start"; reason: "threshold" | "overflow" | "idle" | "incomplete";
	    action: "context-full" | "remote" | "handoff" | "shake" | "snapcompact" }
	| { type: "auto_compaction_end"; action: ...; result: CompactionResult | undefined; aborted: boolean;
	    willRetry: boolean; errorMessage?: string; skipped?: boolean }
	| { type: "auto_retry_start"; attempt: number; maxAttempts: number; delayMs: number;
	    errorMessage: string; errorId?: number }
	| { type: "auto_retry_end"; success: boolean; attempt: number; finalError?: string;
	    retryErrors?: RetryErrorUpdate[] }
	| { type: "retry_fallback_applied"; from: string; to: string; role: string }
	| { type: "retry_fallback_succeeded"; model: string; role: string }
	| { type: "model_changed" }
	| { type: "advisor_cost_changed" }
	| { type: "ttsr_triggered"; rules: Rule[] }
	| { type: "todo_reminder"; todos: TodoItem[]; attempt: number; maxAttempts: number }
	| { type: "todo_auto_clear" }
	| { type: "irc_message"; message: CustomMessage }
	| { type: "notice"; level: "info" | "warning" | "error"; message: string; source?: string }
	| { type: "thinking_level_changed"; thinkingLevel: ThinkingLevel | undefined;
	    configured?: ConfiguredThinkingLevel; resolved?: Effort }
	| { type: "goal_updated"; goal: Goal | null; state?: GoalModeState };
```
and the core `AgentEvent` (`pi-agent-core/src/types.ts:864`, verbatim):
```ts
export type AgentEvent =
	| { type: "agent_start" }
	| { type: "agent_end"; messages: AgentMessage[]; telemetry?: AgentRunSummary; coverage?: AgentRunCoverage }
	| { type: "turn_start" }
	| { type: "turn_end"; message: AgentMessage; toolResults: ToolResultMessage[] }
	| { type: "message_start"; message: AgentMessage }
	| { type: "message_update"; message: AgentMessage; assistantMessageEvent: AssistantMessageEvent }
	| { type: "message_end"; message: AgentMessage }
	| { type: "tool_execution_start"; toolCallId: string; toolName: string; args: any; intent?: string }
	| { type: "tool_execution_update"; toolCallId: string; toolName: string; args: any; partialResult: any }
	| { type: "tool_execution_end"; toolCallId: string; toolName: string; result: any; isError?: boolean };
```

**The frame that carries final assistant text:** `message_end` whose `message.role === "assistant"` (the last one of the turn), and `agent_end.messages[last assistant]`; text is `message.content[].filter(type==="text").text`. On demand: `{"type":"get_last_assistant_text"}` → `{ text: string | null }`.

**Error frames.** There are three, at different layers:
1. Command-level: `{ type:"response", command:"prompt", success:false, error:"…", code?:"…" }` (`rpc-mode.ts:755`). Observed live for the missing-credential case (E.3): `error: "No API key found for anthropic.\n\nUse /login, set an API key environment variable, or create /root/.omp/agent/agent.db"`, no `code`. Note the ordering: a `success:true` for the same `id` is emitted first (prompt accepted), then the failure.
2. Turn-level: the `AssistantMessage` inside `message_end` / `agent_end` (`pi-ai/src/types.ts:933`, relevant fields verbatim):
   ```ts
   export type StopReason = "stop" | "length" | "toolUse" | "error" | "aborted";
   export interface AssistantMessage {
   	role: "assistant"; content: (...)[]; api: Api; provider: Provider; model: string;
   	usage: Usage;
   	stopReason: StopReason;
   	stopDetails?: StopDetails | null;
   	errorMessage?: string;
   	/** Stable recovery-classification text when errorMessage includes display-only diagnostics. */
   	errorClassificationMessage?: string;
   	/** HTTP status surfaced by the provider when the request failed. Populated by every provider's catch block alongside `errorMessage` so consumers (auth retry, telemetry, UI) can branch without regex-scraping the message. */
   	errorStatus?: number;
   	/** Structured machine-readable error classifier; see `utils/error-id.ts` for bit layout and helpers. */
   	errorId?: number;
   	disabledFeatures?: string[];
   	timestamp: number; duration?: number; ttft?: number; completedAt?: number;
   }
   ```
   So **provider** (`provider`, `model`, and `upstreamProvider` for aggregators), **HTTP status** (`errorStatus`) and a **classifier** (`errorId`) are on the frame.
3. Retry-level: `auto_retry_start { attempt, maxAttempts, delayMs, errorMessage, errorId }` and `auto_retry_end { success, attempt, finalError }` — `delayMs` is the cooldown omp itself will sleep, and `finalError` on the fail-fast path is `"Provider requested ${delayMs}ms wait, exceeds retry.maxDelayMs (${maxDelayMs}ms). Original error: …"` (`session/turn-recovery.ts:2221-2229`). A retry-after hint is also appended to the message text as `retry-after-ms=<n>` (`pi-ai/src/utils/retry-after.ts`, `RETRY_AFTER_HINT = "retry-after-ms="`), which `turn-recovery.ts:1923` parses back with `/retry-after-ms\s*[:=]\s*(\d+)/i`.

Lifecycle of the process: `rpc-mode.ts:1532-1545` — when stdin closes, pending requests are failed and the process exits 0. `abort` returns `{success:true}` immediately; the aborted turn ends with an assistant message whose `stopReason` is `"aborted"`.

Also worth noting for the Tauri question (F.5): the protocol has **host tools** — `set_host_tools` registers tools that the *client* executes (`host_tool_call` → `host_tool_result` frames, types `RpcHostToolDefinition`, `RpcHostToolCallRequest`, `RpcHostToolResult` in the same file), and **host URI schemes** (`set_host_uri_schemes`). AETHER could expose `aether_*` to omp over rpc without going through an MCP server at all.

### B.3 `--tools` pinning and MCP tools

Yes on both counts, verified live (E.4-adjacent probes, all in a scratch project containing a
`.mcp.json` that launches a fake stdio MCP server named `aether` exposing `aether_status` and
`aether_search`).

MCP tool names are minted by `src/mcp/tool-bridge.ts`:
```ts
export function createMCPToolName(serverName: string, toolName: string): string {
	const sanitizedServerName = sanitizeMCPToolNamePart(serverName, "server");
	const sanitizedToolName = sanitizeMCPToolNamePart(toolName, "tool");
	// Strip redundant server name prefix from tool name if present
	const prefixWithUnderscore = `${sanitizedServerName}_`;
	let normalizedToolName = sanitizedToolName;
	if (sanitizedToolName.startsWith(prefixWithUnderscore)) {
		normalizedToolName = sanitizedToolName.slice(prefixWithUnderscore.length);
	}
	return capMCPToolNameLength(`mcp__${sanitizedServerName}_${normalizedToolName}`);
}
```
So with the server keyed `aether` in `.mcp.json`, `aether_status` → **`mcp__aether_status`** (the redundant `aether_` is stripped, then re-added by the template). If the server were keyed `aether-mcp`, the name would be `mcp__aether_mcp_aether_status`. Names are capped at 64 chars with a hash suffix.

Probes (cwd = scratch project, `--mode rpc --no-session --model anthropic/claude-sonnet-5`, then `get_state` and reading `dumpTools[].name`):

| `--tools` value | result |
|---|---|
| `read,grep,aether_status` | exit 1, `CliUsageError: Unknown tool in --tools: aether_status. Valid tools: read, grep, write, goal, init_experiment, run_experiment, log_experiment, update_notes, mcp__aether_search, mcp__aether_status.` |
| `read,grep,mcp` | exit 1, same error for `mcp` |
| `read,grep` | `dumpTools: read,grep,write` |
| `read,grep,glob,mcp__aether_status` | `dumpTools: read,grep,glob,mcp__aether_status,write` — MCP entry: `{"name":"mcp__aether_status","description":"Get AETHER local store status (FAKE probe server)","parameters":{"type":"object","properties":{}}}` |
| `bash,edit,read` | `dumpTools: bash,edit,read,write` |
| `--no-tools` | `dumpTools: mcp__aether_search,mcp__aether_status` (MCP tools survive `--no-tools`) |
| `--no-extensions --tools read,grep` | `dumpTools: read,grep,write` |
| `--no-extensions --no-skills --no-rules --tools read,grep,glob,mcp__aether_status` | `dumpTools: read,grep,glob,mcp__aether_status,write` |

Two observations: (1) `write` is **always** force-added even when not requested and even with `--no-extensions` — for a read-only SIR pass AETHER should treat that as a known leak and rely on `--approval-mode`/the fact that the prompt never asks for writes, or block it with an extension `tool_call` hook as the add-on does; (2) the "Valid tools" list in the error is the *currently loaded* set, so it doubles as a discovery command (run once with a bogus name to learn the exact MCP names).

How MCP tools are presented to the model: not as native tool-call definitions but as "devices" addressed by URI. From the captured system prompt: `## MCP Tool Routes … Execute each mounted tool: write JSON arguments to its path. - "aether_status" → \`xd://mcp__aether_status\`` and `## Additional devices (docs on demand) - xd://mcp__aether_status — Get AETHER local store status … Read xd://<tool> for full docs + JSON schema before first use.` A prompt for a SIR pass can therefore say "call `mcp__aether_status`" and the model will resolve it.

### B.4 What `--no-session` still writes

`--no-session` only swaps the session manager for an in-memory one (`main.ts:958 if (parsed.noSession) { return SessionManager.inMemory(); }`); it is rejected in combination with `--fork`/`--from-claude`/`--from-codex`. Observed on disk (fresh `~/.omp`, before/after `find` around an rpc `--no-session` run in the AETHER checkout):

Under `~/.omp` (i.e. `$PI_CODING_AGENT_DIR`'s parent):
- `agent/agent.db` (+ `agent.db-wal`, `agent.db-shm` after the run) — the SQLite store for **auth credentials, credential blocks/refresh leases, settings cache, usage history, model usage/perf, client usage, command usage**. Written on every run (schema/revision rows, usage stats). This is the one file AETHER cannot avoid touching.
- `agent/models.db` — the model catalog cache (`omp models refresh` rebuilds it).
- `logs/omp.<date>.<pid>.log` — one JSON log per process, plus `logs/.omp.<pid>-audit.json`. Written on every run including `--help`.
- `gpu_cache.json` — appeared after the first rpc run (hardware probe cache).
- **Not** written: `agent/sessions/**` (no session file; `get_state.sessionFile` was `undefined`).

Under the project directory: **nothing** (`ls .omp` → no such directory; `git status` clean after the run). omp writes `.omp/` in a project only when a command asks it to (e.g. `omp agents unpack --project`, `/mcp` config edits via `mcp/config-writer.ts` → `.omp/mcp.json`).

Memory-bank / mental-model writes: `memory.backend` defaults to `"off"` (`settings-schema.ts:2948`, confirmed `omp config get memory.backend` → `off`) and `autolearn.enabled` defaults to `false`; hindsight/mnemopi only run when those are set. AETHER's overlay (F.4) should pin them off explicitly so a global setting cannot turn them on.

Isolation knobs: `--profile <name>` moves auth, sessions, settings and caches to `~/.omp/profiles/<name>/` (pi-utils `dirs.ts`); `PI_CODING_AGENT_DIR=<dir>` relocates the agent dir (help text: "Session storage directory (default: ~/.omp/agent)"); `--session-dir` relocates only sessions. **Caution:** a separate profile has its own `agent.db`, i.e. its own logins — AETHER must use the *user's* profile (or none) to ride the subscription credentials.

### B.5 Concurrency

Nothing in omp forbids parallel processes; the shared state and how it is guarded:

- **Auth store (`agent.db`)** is opened by every process. `pi-ai/src/auth/sqlite-credential-store.ts:584` sets `PRAGMA busy_timeout = …` before the first lock-taking statement, uses WAL (`PRAGMA journal_mode=WAL`, line 589), and retries open on `SQLITE_BUSY`/`SQLITE_BUSY_RECOVERY` (lines 523-544). Cross-process races are handled by design, not by a file lock: `auth_credential_refresh_leases` (`#acquireCredentialRefreshLeaseStmt`, `WHERE … expires_at …`) makes OAuth token refresh **single-flight across processes**, and `auth_credential_blocks` persists per-credential usage-limit blocks (`INSERT INTO auth_credential_blocks (credential…` / `SELECT blocked_until_ms, updated_at FROM auth_credential_blocks`) so one process's 429 is visible to the others. `auth_change_revision` + `auth_credential_block_mirror_guard` detect peers rotating rows ("A peer rotated the credential out from under the caller's …", `auth-storage.ts:2482`).
- **Round-robin index** for multi-account providers is **per process, in memory** (`auth-storage.ts:1304 #providerRoundRobinIndex: Map<string, number>`), so N processes do not coordinate which account they start on; each session id is instead hashed to a stable starting account (`#getHashedIndex(sessionId, total) = Bun.hash.xxHash32(sessionId) % total`, line 1747). With one account per provider this is moot.
- **Session files**: none with `--no-session`.
- **Agent registry**: process-global and keyed by `agentId` (matters only in-process, i.e. for the SDK path: the add-on found that concurrent `createAgentSession` calls with the default `"Main"` id evict each other; a subprocess per pass avoids it).
- **Provider-side**: the only real limiter is the subscription's own rate/usage window; omp's `retry.maxRetries=10`, `baseDelayMs=500`, exponential cap `RETRY_BACKOFF_MAX_DELAY_MS = 8_000` per process (`session/retry-fallback-chains.ts:70`) mean 8 parallel processes hitting a 429 will each back off independently.

Verdict for 4–8 parallel `omp --mode rpc --no-session` processes: safe from omp's point of view; the shared sqlite is the only contention point and it is WAL + busy-timeout protected. Keep total concurrency modest because a usage-limit hit on one process blocks the credential for all of them (that is a feature: it stops the others from burning retries).

## C. Quota / rate-limit / auth failure behavior

All paths are relative to `node_modules/@oh-my-pi/`. Three layers handle it: **pi-ai** (HTTP classification, per-request retry, credential rotation), **pi-coding-agent `session/turn-recovery.ts`** (session-level auto-retry, fallback chains, fail-fast), and the **mode runners** (how it reaches stdout/exit code).

### C.1 How 429 / "usage limit" / quota are classified and surface

**Classifier** — `pi-ai/src/error/flags.ts` produces an `errorId` bit-field (`Flag` constants, verbatim excerpt):
```ts
	Transient: 0x0002_0000,
	Timeout: 0x0004_0000,
	UsageLimit: 0x0008_0000,
	StaleResponsesItem: 0x0010_0000,
	MalformedFunctionCall: 0x0020_0000,
	ProviderFinishError: 0x0040_0000,
	EmptyResponse: 0x0000_2000,
	ContentBlocked: 0x0000_8000,
	/** Account-scoped provider policy denial that may succeed with another credential. */
	AccountPolicy: 0x0000_4000,
	ContextOverflow: 0x0080_0000,
	AuthFailed: 0x0100_0000,
	SilentAbort: 0x0200_0000,
	UserInterrupt: 0x0400_0000,
	Abort: 0x0800_0000,
	Grammar: 0x1000_0000,
	FastModeUnsupported: 0x2000_0000,
	/** OAuth refresh failed definitively — the stored grant is dead, re-login required. */
	OAuthExpiry: 0x4000_0000,
	PayloadRejected: 0x8000_0000,
```
The low bits carry the HTTP status (`status(error)`), so `errorId` alone tells a caller "429 + UsageLimit" vs "429 + Transient". Public accessors: `isUsageLimit(error)`, `isAccountPolicyError(error)`, `retriable(id)`, `classify(error, api)`.

**Usage-limit phrasing table** — `pi-ai/src/error/rate-limit.ts:275`, verbatim:
```ts
const USAGE_LIMIT_PATTERN =
	/usage.?limit|usage_limit_reached|usage_not_included|limit_reached|quota.?(?:exceeded|reached|insufficient)|额度不足|额度耗尽|resource.?exhausted|exhausted your capacity|quota will reset|insufficient.?(?:balance|quota)|balance.?exhausted|run out of credits|out of credits|spending[- _]?limit|personal-team-blocked/i;
```
plus `SPEND_LIMIT_PATTERN = /spend.?limit/i`, `ACCOUNT_RATE_LIMIT_PATTERN = /\baccount(?:'s)?\b[^\n]{0,80}\brate.?limit\b|…/i`, `SUBSCRIPTION_CAP_PATTERN = /\b(?:subscription|plan|membership)\b[^\n]{0,80}\b(?:rate.?limits?|quota|cap)\b|…/i` (excluding `per second|per minute`), `OPENROUTER_DAILY_FREE_LIMIT_PATTERN`, Chinese-language quota patterns, Google `google.rpc.ErrorInfo` structured reasons, and `isUsageLimitStatus(status) = status === 429 || status === 402`. The decision tree is documented in the file:
```
 *  1. Body matches isUsageLimitError (Codex `usage_limit_reached`, Anthropic account rate-limit,
 *     Google `resource_exhausted`, OpenAI `insufficient_quota`, …) → rotate.
 *  2. Status is not a usage-limit status (429/402) → backoff (caller's domain).
 *  3. Body is absent or opaque (just the status, empty JSON, HTTP framing only) → rotate conservatively
 *  4. Body has content → defer to parseRateLimitReason. `QUOTA_EXHAUSTED` rotates; … `RATE_LIMIT_EXCEEDED`
 *     (`Too many requests`, per-minute caps), `MODEL_CAPACITY_EXHAUSTED` (`Service overloaded`),
 *     `SERVER_ERROR`, and `UNKNOWN` (`Please retry in 5s`) stay in the provider's own backoff layer
```
Reason → backoff constants in the same file: `QUOTA_EXHAUSTED_BACKOFF_MS = 30 min`, `RATE_LIMIT_EXCEEDED_BACKOFF_MS = 30 s`, `CONCURRENT_LIMIT_BACKOFF_MS = 5 s`, `MODEL_CAPACITY_BASE_MS = 45 s ± 15 s`, `SERVER_ERROR_BACKOFF_MS = 20 s`. The Codex "You have hit your ChatGPT usage limit … Try again in ~158 min." phrasing is explicitly handled (`error/gateway.ts` comment) and the "session limit" wording you asked about is not a distinct pattern: it is caught by `usage.?limit` / `limit_reached` / the subscription-cap pattern.

**Transient vs terminal** — `error/retryable.ts`: `isTransientStatus = 408 || 429 || >= 500`; `isProviderRetryableError()` returns **false for usage limits** ("they are owned by the credential-rotation layer … not this seconds-scale provider backoff"), true for 429-transient, 5xx, socket resets, `TRANSIENT_TRANSPORT_PATTERN` (`overloaded|rate.?limit|too many requests|429|500|502|503|504|service.?unavailable|…`).

**Retry-after parsing** — `pi-ai/src/utils/retry-after.ts`: reads `retry-after-ms`, `retry-after` (seconds or HTTP-date), `x-ratelimit-reset-ms`, `x-ratelimit-reset` (ms/s/epoch heuristics), takes the **max**, and appends `retry-after-ms=<n>` to the error message (`formatErrorMessageWithRetryAfter`) so the hint survives into `errorMessage`.

**Error classes** (`error/classes.ts`): `ProviderHttpError { status }` with subclasses `OpenAIHttpError`, `AnthropicApiError`, `BedrockApiError`, `GeminiCliApiError`, `GoogleApiError`, `OllamaApiError`, `AuthGatewayError`; plus transport errors (`AnthropicConnectionError`, `CodexWebSocketTransportError`, …). There is no dedicated `retryAfterMs` field on the class; the hint travels in the message.

**How it surfaces**:
- **rpc mode** and **`-p --mode json`**: identical, because both write every `AgentSessionEvent`. A quota hit produces, in order: `auto_retry_start {attempt, maxAttempts, delayMs, errorMessage, errorId}` (repeated per attempt, or `retry_fallback_applied` if a chain is configured), then either `auto_retry_end {success:true}` or `auto_retry_end {success:false, finalError}` followed by `message_end`/`agent_end` whose assistant message has `stopReason:"error"`, `errorStatus:429`, `errorId` with `UsageLimit`, and `errorMessage` ending in `retry-after-ms=<n>` when the provider sent a header. Exit code stays **0** (rpc) / **0** (json) — see B.1.
- **`-p` text mode**: only the final `errorMessage` on stderr and exit **1**; retry progress is invisible.
- **Pre-loop auth failure** (no credential, expired grant → `Flag.OAuthExpiry`): rpc → `{"type":"response","command":"prompt","success":false,"error":"No API key found for …"}`; `-p` → uncaught exception, exit 1, stack on stderr (E.2).

### C.2 Round-robin rotation and per-credential backoff

Lives in **`pi-ai/src/auth-storage.ts`** (`AuthStorage`, header comment: "credential management with round-robin, usage-limit blocking…"):
```ts
	/** Composite key for round-robin tracking: "anthropic:oauth" or "openai:api_key" */
	#getProviderTypeKey(provider: string, type: AuthCredential["type"]): string { return `${provider}:${type}`; }
	#getNextRoundRobinIndex(providerKey: string, total: number): number {
		if (total <= 1) return 0;
		const current = this.#providerRoundRobinIndex.get(providerKey) ?? -1;
		const next = (current + 1) % total;
		this.#providerRoundRobinIndex.set(providerKey, next);
		return next;
	}
	/** FNV-1a hash for deterministic session-to-credential mapping. */
	#getHashedIndex(sessionId: string, total: number): number {
		if (total <= 1) return 0;
		return Bun.hash.xxHash32(sessionId) % total;
	}
	/**
	 * With sessionId: starts from hashed index (consistent per session).
	 * Without sessionId: starts from round-robin index (load balancing).
	 * Order wraps around so all credentials are tried if earlier ones are blocked.
	 */
```
It applies to **both** OAuth accounts and API keys, keyed separately per `provider:type` — i.e. rotation is *within a provider* and never across providers. Per-credential blocks are set by `markUsageLimitReached(provider, sessionId, { retryAfterMs, baseUrl, modelId, … }): Promise<UsageLimitMarkResult>`; for OAuth credentials it additionally queries the provider's **usage report** and extends the block to the window's `resetsAt`:
```ts
		let blockedUntil = now + (options?.retryAfterMs ?? AuthStorage.#defaultBackoffMs);
		if (credentialType === "oauth" && target.credential.type === "oauth" && routing.strategy) {
			const report = await raceUsageWithSignal(this.#getUsageReport(provider, target.credential, options), options?.signal);
			if (report) {
				const scopedLimits = this.#getScopedUsageLimits(routing.strategy, report, routing.rankingContext);
				if (this.#isUsageLimitReached(scopedLimits)) {
					const resetAtMs = this.#getUsageResetAtMs(scopedLimits, Date.now());
					if (resetAtMs && resetAtMs > blockedUntil) blockedUntil = resetAtMs;
```
Blocks are persisted to `agent.db` table `auth_credential_blocks` (`sqlite-credential-store.ts:457-478`, `blocked_until_ms`), so they are **shared across processes and survive restarts**. OAuth refresh is single-flighted across processes by `auth_credential_refresh_leases`. The `auth-retry.ts` driver decides refresh-same vs rotate-sibling ("Ordinary 401/auth failures retain one refresh-same plus one sibling switch; 403/usage-limit failures enter sibling rotation"). Usage-report failures back off 10 s (`USAGE_FAILURE_BACKOFF_MS = 10_000`).

### C.3 Session-level retry, fallback chains, and how a retry that finally fails surfaces

Settings (`pi-coding-agent/src/config/settings-schema.ts:1781-1900`, defaults confirmed by `omp config get`): `retry.enabled=true`, `retry.maxRetries=10`, `retry.baseDelayMs=500`, `retry.maxDelayMs=300000` ("When the provider asks us to wait longer than this and no credential or model fallback succeeds, the request fails fast instead of sleeping (e.g. 3-hour Anthropic rate-limit windows). 0 disables the ceiling"), `retry.modelFallback=true` ("Allow retry recovery to switch to configured fallback models"), `retry.usageAwareFallback=false`, `retry.usageReservePct=10`, `retry.usageReservePolicy="confirm"`, `retry.fallbackRevertPolicy="cooldown-expiry"`, and:
```ts
	"retry.fallbackChains": {
		type: "record",
		default: {} as Record<string, string[]>,
		ui: { … description:
			'JSON object mapping model roles, model selectors ("provider/model-id"), or provider wildcards ("provider/*") to ordered fallback selectors, e.g. {"default":["openai/gpt-4o-mini"],"google-antigravity/*":["google/*","google-vertex/*"]}. Model-oriented keys apply whenever that model/provider is active, regardless of role; a "provider/*" entry keeps the failing model\'s id and swaps the provider. …' },
	},
```
Implementation: `session/retry-fallback-chains.ts` (resolution, `expandDefaultRetryFallbackChains` applies a `"default"` chain to every role) and `session/turn-recovery.ts` (`#maybeApplyUsageAwareFallback` at 1459 gated on `retry.usageAwareFallback`; chain walking at ~1795-1860 gated on `retrySettings.enabled && retrySettings.modelFallback`; emits `retry_fallback_applied {from, to, role}`). **With `fallbackChains` empty nothing can switch model or provider**; `modelFallback=true` is inert. `retry.fallbackRevertPolicy` only matters once a fallback happened.

Fail-fast path (`turn-recovery.ts:2214-2245`, verbatim):
```ts
		const maxDelayMs = retrySettings.maxDelayMs;
		if (maxDelayMs > 0 && delayMs > maxDelayMs && !switchedCredential && !switchedModel) {
			await this.persistTerminalEmptyErrorTurn(message);
			const attempt = this.#retryAttempt;
			this.#retryAttempt = 0;
			await this.#host.emitSessionEvent({
				type: "auto_retry_end", success: false, attempt,
				finalError: `Provider requested ${delayMs}ms wait, exceeds retry.maxDelayMs (${maxDelayMs}ms). Original error: ${errorMessage}`,
			});
			…
			return false;
		}
		await this.#recordPendingRetryError(message, id, { switchedCredential, switchedModel, delayMs });
		await this.#host.emitSessionEvent({ type: "auto_retry_start", attempt: this.#retryAttempt, maxAttempts: maxRetries, delayMs, errorMessage, errorId: message.errorId });
```
`delayMs` is the larger of exponential backoff (cap 8 s), the reason-based backoff (30 min for QUOTA_EXHAUSTED), the parsed `retry-after-ms`, and, for usage limits, `UsageLimitMarkResult.retryAtMs` (earliest sibling unblock) — so a subscription window exhaustion normally trips the 5-minute fail-fast immediately and the process returns the error instead of sleeping.

### C.4 Signals a caller can read for pause-and-resume

In rpc / json mode, in order of quality:
1. `auto_retry_end.finalError` matching `Provider requested (\d+)ms wait, exceeds retry.maxDelayMs` — gives the exact wait omp computed (includes provider `retry-after` and the account window `resetsAt`).
2. `auto_retry_start.delayMs` — while omp is still retrying; `errorId & 0x0008_0000` (UsageLimit) says whether it is a window exhaustion vs a transient.
3. Final `message_end`/`agent_end` assistant message: `errorStatus` (429/402/401/403), `errorId`, and `retry-after-ms=<n>` at the end of `errorMessage`.
4. Out of band: `omp usage --json` (per-account usage windows with `resetsAt`, from the same `#getUsageReport` the rotation uses) — could not be exercised here (`No credentials found`, exit 1). `omp usage --help` lists `--json`, `--provider`, `--history --days N`.
5. Config: setting `retry.maxDelayMs: 0` in AETHER's overlay would instead make omp **sleep through** the window itself (auto-resume), which is the other valid design.

In `-p` text mode none of this is available except the final stderr line and exit 1.

## D. MCP and rules discovery

### D.1 Does omp load AETHER's `.mcp.json`?

Yes, when present. Provider `src/discovery/mcp-json.ts` ("Discovers standalone mcp.json / .mcp.json files in the project root"): `const filenames = ["mcp.json", ".mcp.json"]` joined to `ctx.cwd`, gated by setting `mcp.enableProjectConfig` (default `true`, description "Load .mcp.json/mcp.json from project root"). Other MCP sources omp also reads (all in `src/discovery/`): `.omp/mcp.json` (native, `mcp/config-writer.ts`), `.claude/.mcp.json` / `.claude/mcp.json` and `~/.claude/mcp.json` (`claude.ts:64-68`), `.cursor/mcp.json` + `~/.cursor` (`cursor.ts`), `.gemini/settings.json` + `~/.gemini/settings.json` (`gemini.ts`), Codex config (`codex.ts`). Servers are merged by name; the MCP handshake identifies as `clientInfo: {"name":"omp-coding-agent","version":"1.0.0"}` with `protocolVersion "2025-11-25"`.

In the AETHER checkout `.mcp.json` is **absent** (it is gitignored: `.gitignore:18 .mcp.json`, `:19 .claude/`), so the in-repo rpc run showed no `mcp__*` tools (`dumpTools: read,bash,edit,eval,glob,grep,task,hub,todo,web_search,write`). Live proof was therefore done in a scratch project (E.4): the fake server's log recorded the `initialize`, `notifications/initialized`, `tools/list` sequence and the tools appeared as `mcp__aether_status` / `mcp__aether_search`. Since `.claude/` is also gitignored, on your dev box omp will pick up whatever `.mcp.json` you have locally; nothing else in the repo (`CLAUDE.md`'s "MCP Binary Hint: ./target/debug/aether-mcp" is prose, not config).

### D.2 `.claude/commands/*.md` vs `.claude/skills`

Both are imported:
- **Commands**: `src/discovery/claude.ts:300-345 loadSlashCommands()` scans `~/.claude/commands/**/*.md` and `<cwd>/.claude/commands/**/*.md` (recursive), name = path relative to the commands dir minus `.md`, nested dirs get namespace aliases; toggles `commands.enableClaudeUser` / `commands.enableClaudeProject` (both default true). Verified live: a scratch `.claude/commands/audit.md` produced `audit(file) — Guided audit workflow (probe)` in `get_available_commands`, i.e. **`/audit` exists**. The AETHER clone here has no `.claude/commands/` (only `.claude/skills/`), so the real `audit.md` could not be tested; the mechanism is the same. `$ARGUMENTS` substitution is handled in `src/extensibility/slash-commands.ts:132`.
- **Skills**: `.claude/skills/*/SKILL.md` (walking up from cwd) and `~/.claude/skills` → commands named `skill:<name>` (`skills.enableClaudeProject`, `skills.enableSkillCommands`, default true). In the AETHER checkout the four repo skills appeared: `skill:aether-build, skill:dashboard-polish, skill:stage-workflow, skill:validation-gates` (plus `skill:startup-hook-skill` from the container's `~/.claude`). Codex `.codex/commands` + `~/.codex/commands` and `.omp/commands` are also scanned.

### D.3 Which context files omp actually reads, and precedence

Observed in the AETHER checkout (`get_state.systemPrompt`, 2 segments, 27,012 chars): the root **`AGENTS.md` is injected verbatim** under `<repo-rules>MUST follow these context files for all tasks: <file path="/home/user/aether/AGENTS.md">…`. The root **`CLAUDE.md` was not** (no "AETHER Code Intelligence" / "Agent Schema Version" text anywhere in the prompt). In the scratch project with all five marker files: `AGENTS.md` → **yes**; `CLAUDE.md` (root) → no; `GEMINI.md` (root) → no; `.cursor/rules/probe.mdc` (`alwaysApply: true`) → no; `.cursorrules` → no.

Source: `src/discovery/agents-md.ts` walks up from cwd to the git root collecting `AGENTS.md` (stopping before `$HOME`); `src/discovery/claude.ts:128-160 loadContextFiles()` reads only `~/.claude/CLAUDE.md` and `<cwd>/.claude/CLAUDE.md` — **not** `<cwd>/CLAUDE.md`; `src/discovery/gemini.ts` reads `~/.gemini/GEMINI.md` and `.gemini/GEMINI.md`; `src/discovery/codex.ts` reads `~/.codex/AGENTS.md`; `src/discovery/cursor.ts` loads `.cursor/rules/*.mdc` as *rules* (glob-scoped, `capability/rule.ts` has `globs`/`alwaysApply`), which are applied via the TTSR/rule engine rather than pasted into the system prompt — the probe shows an `alwaysApply` rule still does not land in `systemPrompt`. `--no-rules` disables rules discovery. Precedence for context files is concatenation (user-level then project-level, deeper directories later), not first-wins.

Practical consequence for AETHER: the agent guidance that matters for omp is `AGENTS.md`; the MCP tool list in `CLAUDE.md` is invisible unless duplicated into `AGENTS.md` or `.claude/CLAUDE.md`, or passed with `--append-system-prompt`.

## E. Live probes

Setup common to all probes: `omp` = `/opt/node22/bin/bun <scratch>/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js` (18.0.10, Bun 1.4.2), fresh `~/.omp` (no logins, no `~/.omp/agent/config.yml`), no `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`/`GEMINI_API_KEY` in the environment. Every command was run with stdin closed (`</dev/null`) unless stated; see E.2 for why.

### E.1 `omp models`

```
$ omp --version
omp/18.0.10
$ omp usage
No credentials found. Run `omp` and use /login to add accounts.
(exit=1)
$ omp models
amazon-bedrock (145)
┌──────────────────────────────────────────────────┬─────────┬─────────┬───────────────────────────┬────────┐
│ model                                            │ context │ max-out │ thinking                  │ images │
├──────────────────────────────────────────────────┼─────────┼─────────┼───────────────────────────┼────────┤
│ anthropic.claude-3-5-haiku-20241022-v1:0         │    200K │    8.2K │ -                         │ yes    │
│ …                                                                                                       │
│ anthropic.claude-opus-5                          │      1M │    128K │ low,medium,high,max       │ yes    │
│ anthropic.claude-sonnet-5                        │      1M │    128K │ low,medium,high,max       │ yes    │
│ global.anthropic.claude-fable-5                  │      1M │    128K │ low,medium,high,max       │ yes    │
│ global.openai.gpt-5.6-luna                       │    1.1M │    128K │ low,medium,high,xhigh,max │ yes    │
│ …
(exit=1)
```
Only `amazon-bedrock` (145) and `bedrock-mantle` (5) were listed as *available*, because the container exports `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` for its own infrastructure (I did not use them for inference). **No subscription provider (anthropic, openai-codex, google-antigravity/gemini) is logged in here**, so E.2 and E.4 could not reach a model. `omp models --json` returns `{"models":[{"provider","id","selector":"provider/id","name","contextWindow","maxTokens","reasoning","thinking","input","cost"}…]}` — note `selector` is exactly the `--model` string to use.

### E.2 `omp -p` per provider

No logged-in provider, so one representative call per mode against `anthropic/claude-sonnet-5`, recording the auth-failure path:

First attempt (stdin left as the harness pipe) — both modes **hung until the 120 s timeout**:
```
$ omp -p --no-session --model anthropic/claude-sonnet-5 "Reply with the single word OK"
exit=124 wall=120.01s   stdout: (0 bytes)
stderr: Reading prompt from piped stdin (waiting for EOF; ctrl+c to abort)…
        Still starting after 10s — phase: readPipedInput
          logs: /root/.omp/logs/omp.2026-09-10.3409.log · re-run with PI_DEBUG_STARTUP=1 for streaming phase markers
        … (repeats every 10 s)
```
Same for `--mode json`, for a bad model id, and for an unknown flag: nothing is evaluated until stdin reaches EOF. **Any spawner must close stdin or write the prompt to it and close.**

With `</dev/null`:
```
$ omp -p --no-session --model anthropic/claude-sonnet-5 "Reply with the single word OK" </dev/null
exit=1 wall=13.93s
stdout: (0 bytes)
stderr (3077 bytes): Working...
  17374 | [Output truncated. Showing first ${IOt.toLocaleString()} characters.]`;try{let{path:n,id:s}=await this.sessionManager.allocateArtifactPath("async");…
  … (≈5 lines of minified bundle source excerpt)
  17379 | Then use /model to select a model.`);if(!await this.#pe.getApiKey(this.model,this.sessionId))throw Error(`No API key found for ${this.model.provider}.
                                                                                                                ^
  error: No API key found for anthropic.

  Use /login, set an API key environment variable, or create /root/.omp/agent/agent.db
        at #Xn (…/pi-coding-agent/dist/cli.js:17379:105)
        at async prompt (…/pi-coding-agent/dist/cli.js:17375:36095)

$ omp -p --mode json --no-session --model anthropic/claude-sonnet-5 "Reply with the single word OK" </dev/null
exit=1 wall=2.32s
stdout (140 bytes): {"type":"session","version":3,"id":"01a08b88-cf08-77ca-8ff3-c349d72c4331","timestamp":"2026-09-10T13:36:34.056Z","cwd":"/home/user/aether"}
stderr (3066 bytes): (same bun source excerpt + "error: No API key found for anthropic. …")

$ omp -p --no-session --model anthropic/does-not-exist-9 "Reply OK" </dev/null
exit=1 wall=2.30s
stderr: Model "anthropic/does-not-exist-9" not found

        Set an API key environment variable:
          ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY, etc.

        Or create /root/.omp/agent/models.yml

$ omp -p --no-session --model nosuchprovider/foo "Reply OK" </dev/null
exit=1 wall=2.52s   stderr: Model "nosuchprovider/foo" not found … (same hint)

$ omp -p --no-session --list-models "x" </dev/null
exit=2 wall=1.48s   stderr: Error: unknown flag: --list-models
                            Run `omp --help` for available flags.

$ omp -p --no-session --model anthropic/claude-sonnet-5 </dev/null        # no prompt at all
exit=0 wall=1.92s   stdout: (0 bytes)  stderr: (0 bytes)

$ echo "Reply with the single word OK" | omp -p --mode json --no-session --model anthropic/claude-sonnet-5
exit=1 wall=1.71s   stdout: {"type":"session",…,"cwd":"/home/user/aether"}
                    stderr: … error: No API key found for anthropic. …
```
The 13.9 s on the first text-mode run is cold-start (model catalog + extension discovery); subsequent runs were 1.7–2.5 s. The bun source excerpt on stderr is noise from a missing sourcemap in the published bundle; AETHER should take the first line after `error:` as the message.

### E.3 Full `omp --mode rpc --no-session` exchange

Run from `/home/user/aether` with a feeder script (6 s startup wait, then one frame per second; `abort` 8 s after the prompt):
```
$ ./rpc_feed.sh | omp --mode rpc --no-session --model anthropic/claude-sonnet-5
exit=0 wall=22.05s   stderr: (empty)
```
stdin frames sent:
```
{"id":"1","type":"negotiate_protocol","protocolVersion":2}
{"id":"2","type":"get_state"}
{"id":"3","type":"get_available_commands"}
{"id":"4","type":"get_available_models"}
{"id":"5","type":"prompt","message":"reply OK"}
{"id":"6","type":"abort"}
{"id":"7","type":"get_last_assistant_text"}
```
stdout frames received, in order (long ones truncated with `…`):
```
{"type":"ready","protocolVersion":1,"supportedProtocolVersions":[1,2],"maxFrameBytes":1048576,"maxReassembledFrameBytes":67108864}
{"type":"extension_ui_request","id":"1579e5c47e1065e5","method":"setWidget","widgetKey":"autoresearch"}
{"type":"available_commands_update","commands":[{"name":"security","description":"Plan, run, inspect, import, and compare OMP-native security scans",…,"source":"builtin"},{"name":"model","aliases":["models"],…}, … 47 commands …]}
{"id":"1","type":"response","command":"negotiate_protocol","success":true,"data":{"protocolVersion":2}}
{"id":"2","type":"response","command":"get_state","success":true,"data":{"model":{"id":"claude-sonnet-5","name":"Claude Sonnet 5","api":"anthropic-messages","provider":"anthropic","baseUrl":"https://api.anthropic.com","reasoning":true,"input":["text","image"],"cost":{"input":2,"output":10,"cacheRead":0.2,"cacheWrite":2.5},"contextWindow":1000000,"maxTokens":128000,"thinking":{"mode":"anthropic-adaptive","efforts":["low","medium","high","xhigh","max"],"supportsDisplay":true},…},"thinkingLevel":"high","isStreaming":false,"isCompacting":false,"steeringMode":"one-at-a-time","followUpMode":"one-at-a-time","interruptMode":"immediate","sessionId":"01a08a61-c125-7172-afbc-d38a3b783ddb","autoCompactionEnabled":true,"queuedMessageCount":0,"todoPhases":[],"fastModeEnabled":false,"tokensPerSecond":null,"fastModeActive":false,"messageCount":0,"systemPrompt":["<system-conventions>\nRFC 2119: …", "…<repo-rules>\nMUST follow these context files for all tasks:\n<file path=\"/home/user/aether/AGENTS.md\">\n# AETHER — Codex Context …"],"dumpTools":[{"name":"read",…},{"name":"bash",…},…]}}
{"id":"3","type":"response","command":"get_available_commands","success":true,"data":{"commands":[… 47 …]}}
{"id":"4","type":"response","command":"get_available_models","success":true,"data":{"models":[{"id":"anthropic.claude-3-5-haiku-20241022-v1:0","name":"Claude Haiku 3.5","api":"bedrock-converse-stream","provider":"amazon-bedrock",…}, … 150 …]}}
{"id":"5","type":"response","command":"prompt","success":true}
{"id":"5","type":"response","command":"prompt","success":false,"error":"No API key found for anthropic.\n\nUse /login, set an API key environment variable, or create /root/.omp/agent/agent.db"}
{"id":"6","type":"response","command":"abort","success":true}
{"id":"7","type":"response","command":"get_last_assistant_text","success":true,"data":{}}
{"type":"extension_ui_request","id":"1579e5d8bfd065e6","method":"setWidget","widgetKey":"autoresearch"}
```
Derived facts: no `sessionFile` in state (ephemeral confirmed); `get_last_assistant_text` returned `data: {}` (the `text` key is omitted when null); `dumpTools` = `read,bash,edit,eval,glob,grep,task,hub,todo,web_search,write`; 47 commands, sources `builtin, skill, extension, custom, file`, including `skill:aether-build, skill:dashboard-polish, skill:stage-workflow, skill:validation-gates` from the repo's `.claude/skills`; the system prompt contained `AGENTS.md` but not root `CLAUDE.md`. The `extension_ui_request … setWidget` frames come from the bundled `autoresearch` extension; `--no-extensions` removes them. Process exited 0 on stdin EOF.

Files written by this run (diff of `find ~/.omp -type f` before/after): `agent/agent.db-shm`, `agent/agent.db-wal`, `gpu_cache.json`, `logs/omp.2026-09-10.3067.log`, `logs/.omp.3067-audit.json`. Nothing under `/home/user/aether` (`git status` clean, no `.omp/`).

### E.4 MCP tool call via `-p`

Not executable: no model credential, and no `aether-mcp` binary (the repo is unbuilt here; `./target/debug/aether-mcp` does not exist and building the 17-crate workspace was out of scope). What *was* verified with a stand-in stdio server named `aether` exposing `aether_status`/`aether_search` in a scratch project's `.mcp.json`:
```
$ ./rpc_feed2.sh | omp --mode rpc --no-session --model anthropic/claude-sonnet-5      # cwd = scratch project
exit=0 ; stderr: (empty)
fake MCP server log:
  REQ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{"roots":{"listChanged":false}},"clientInfo":{"name":"omp-coding-agent","version":"1.0.0"}}}
  REQ {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
  REQ {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
get_state → systemPrompt contains:
  ## MCP Tool Routes
  Execute each mounted tool: write JSON arguments to its path.
  - "aether_search" → `xd://mcp__aether_search`
  - "aether_status" → `xd://mcp__aether_status`
```
and `--tools read,grep,glob,mcp__aether_status` → `dumpTools: read,grep,glob,mcp__aether_status,write` (full table in B.3). Whether the *model* actually issues the call could not be observed without a provider.

### E.5 Quota / rate-limit samples

None captured from omp — no provider could be reached. The only 429 seen during this session was against this Claude Code session's own API quota (three research subagents died with "You've hit your session limit · resets 12:30pm (UTC) (error type rate_limit, HTTP 429)"), which is unrelated to omp. The real sample must come from the dev box; the frame shapes to look for are in C.1/C.4.

## F. Verdicts (my opinion, clearly labeled as such)

### F.1 `OmpProvider`: rpc mode or `-p` mode?

**Opinion: rpc mode (`omp --mode rpc --no-session --no-extensions`), one long-lived process per worker slot, not one `-p` process per symbol.** Reasons, in order of weight:
1. Error discrimination only exists in the frame stream. `-p` text mode collapses everything to exit 1 + one stderr line (and a bun stack). `-p --mode json` gives the frames but exits 0 on a mid-turn error and 1 on a pre-loop error, so AETHER would parse frames anyway — at which point rpc is the same parser with a persistent process.
2. Startup cost: 1.7–2.5 s warm, ~14 s cold per `-p` process (catalog load, extension/skill/MCP discovery, MCP server handshake). For a deep pass over thousands of symbols that is the dominant cost; rpc amortizes it and keeps the MCP connection to `aether-mcp` open.
3. rpc has `set_model` / `set_thinking_level` per prompt, `abort`, `get_session_stats` (tokens, cost, `premiumRequests`), and `get_last_assistant_text` — exactly the control surface the batch runner needs — and `new_session` to reset context between symbols without a respawn.
4. rpc lets AETHER register `aether_*` as **host tools** (`set_host_tools`) executed by the Rust side, so the batch pipeline does not need a separate `aether-mcp` process or `.mcp.json` at all; SurrealKV's exclusive lock stops being a deployment constraint.
5. The add-on's own `SubprocessAdapter` documents the `-p` limitations ("print mode cannot resume a session … the model sees a paraphrased history") and calls itself "Debug-only fallback".

Keep `-p --mode json` as the *verification* path (single symbol, reproducible command line) and for `scan_all.sh`-style shell use (F.6).

### F.2 Per-pass model selection (scan / triage / deep)

**Opinion: explicit `provider/model[:thinking]` strings, sent as `--model` at spawn and `set_model` + `set_thinking_level` per prompt in rpc. Do not use omp roles (`@smol/@slow/@plan`) and do not generate a per-workspace `config.yml` for model selection.** Reasons: the `provider/id` form bypasses fuzzy matching (a bare `"sonnet"` could resolve to bedrock or anthropic depending on login state); it maps 1:1 onto `BatchConfig.resolve_model(pass, provider)` / `resolve_thinking` which already produce per-pass strings; omp roles are user-global settings that your interactive sessions also read, so repurposing `smol/slow/plan` for AETHER passes would change your own omp defaults. `--thinking` values (`off, minimal, low, medium, high, xhigh, max, auto`) are a superset of AETHER's (`off/none/minimal/low/medium/high/dynamic`): map `none→off`, `dynamic→auto`. Validate a selector before a run by spawning `omp -p --no-session --model <sel> </dev/null` (exit 1 + `Model "…" not found` in 2 s), or better via rpc `get_available_models` filtered by `provider`.

### F.3 Pause-and-resume: real signal or exit-code + pattern matching?

**Opinion: a real signal is available in rpc/json mode; pattern matching is only needed as a last-resort fallback.** Key off, in priority order: (a) `auto_retry_end{success:false, finalError:"Provider requested <N>ms wait, exceeds retry.maxDelayMs …"}` — parse `N` and sleep `N`; (b) `auto_retry_start{delayMs, errorId}` with `errorId & 0x0008_0000` (UsageLimit) — omp is about to sleep `delayMs` itself; (c) the terminal assistant message's `errorStatus === 429 || 402` plus `retry-after-ms=<n>` suffix in `errorMessage`. Set `retry.maxRetries: 1` (or small) and keep `retry.maxDelayMs` at its 5-minute default in the overlay so the pause decision comes back to AETHER quickly instead of omp sleeping inside the worker; alternatively set `retry.maxDelayMs: 0` and let omp itself sleep through the window, which is simpler but makes the worker look hung (no frame is emitted while it waits except the initial `auto_retry_start`). Exit code alone is useless (0 for a mid-turn quota failure in json/rpc); `-p` text mode is exit-1 + regex only.

### F.4 Guaranteeing no fallback chains fire, scoped to AETHER's invocations

**Opinion: pass a per-invocation overlay with `--config <aether-overlay.yml>`; that is path-scoped to the process and never touches `~/.omp/agent/config.yml` or the project's `.omp/config.yml`.** Settings precedence is global → project → overlay (`config/settings.ts:1291` "global → project → overrides; project wins over global"; overlays are loaded after both, `#loadConfigOverlays()`; missing/malformed overlay files are hard errors). Minimal overlay content AETHER should generate into its own workspace dir (e.g. `.aether/omp-overlay.yml`):
```yaml
retry:
  fallbackChains: {}          # no cross-model/provider chains (schema default, pinned)
  modelFallback: false        # chain walking disabled even if a global chain exists
  usageAwareFallback: false   # no quota-driven switching to other accounts/models
  maxRetries: 1               # surface 429s to AETHER instead of omp sleeping (see F.3)
  # maxDelayMs: 300000        # default; set 0 to make omp sleep through windows instead
memory:
  backend: off
autolearn:
  enabled: false
mcp:
  enableProjectConfig: false  # if aether_* are supplied as host tools; true if via .mcp.json
```
Caveats: (1) omp still rotates **between accounts of the same provider** if you have several logged in; that is within-provider and arguably fine under #127, but if it is not, there is no setting to stop it short of logging only one account in (`omp token <provider> --list` shows how many). (2) `providers.anthropic.serverSideFallback` (Fable 5 → Opus 4.8 server-side on classifier block) defaults to `false`; pin it explicitly too. (3) `--profile` would isolate settings completely but also isolates **logins**, so it defeats the subscription goal — do not use it. (4) The overlay cannot stop a `.claude/settings.json` or `.omp/config.yml` in the *project* from being read, but it overrides whatever they set.

### F.5 Tauri app: embed omp via rpc / Node SDK instead of a custom orchestrator?

**Opinion: yes for the agent loop, via rpc (spawn `omp --mode rpc-ui` or `rpc` as a sidecar from the Rust side), not the Node SDK. The biggest blocker is distribution, not protocol.** The rpc surface already covers what a desktop agent panel needs: streaming `message_update` deltas, tool lifecycle events, `extension_ui_request` (select/confirm/input/notify/open_url for OAuth), `login`/`get_login_providers` (so the Tauri app could drive `/login` itself), `set_host_tools` (AETHER's intelligence tools executed in-process by the Tauri backend against the already-open `SharedState` — no second store handle, no SurrealKV lock conflict), `compact`, `branch`, `export_html`, `get_session_stats`. The Node SDK (`createAgentSession`, `discoverAuthStorage`, `ModelRegistry`) would force a Bun/Node runtime inside the Tauri bundle and is the path the add-on uses because it is *already* JavaScript; for a Rust app the subprocess is the natural boundary. Blockers/costs: omp is a ~30 MB Bun bundle plus native modules (`pi-natives-linux-x64`) with `engines.bun >= 1.3.14` — the app must either require a user-installed omp (and bun), or bundle both; the current AETHER Phase 9 docs explicitly say "no subprocess management" and "Sidecar bundling (not needed — we embed `aetherd` directly)" (`phase_9_beacon.md:37,51`), so this is a documented architectural reversal for the agent feature only; rpc-mode `extension_ui_request` frames need a UI mapping; and the protocol is versioned but not frozen (`supportedProtocolVersions: [1,2]`, types live in the package, not a published schema). Also note omp's `--mode acp` (Agent Client Protocol) exists as a second, standardized option if you would rather depend on a public protocol than on omp's rpc types.

### F.6 Porting `scan_all.sh` / `enrich_all.sh` (`claude -p --allowedTools "mcp__aether*"`) 1:1

**Opinion: yes, nearly 1:1, with three deltas.** The scripts themselves are not in the repo clone, so this is based on the prompt's description of them. Replacement command line:

```bash
# claude -p "<prompt>" --allowedTools "mcp__aether*"
omp -p --mode json --no-session --no-extensions \
    --model anthropic/claude-sonnet-5:high \
    --tools read,grep,glob,mcp__aether_status,mcp__aether_search,mcp__aether_sir_context \
    --approval-mode yolo \
    --append-system-prompt "$(cat .aether/sir-pass-prompt.md)" \
    "<prompt>" </dev/null \
  | jq -r 'select(.type=="message_end" and .message.role=="assistant") | .message.content[] | select(.type=="text") | .text' \
  | tail -n 1
```
Deltas: (1) **no wildcard** — `--tools` takes exact names, and MCP names are `mcp__<server>_<tool>` with the server prefix de-duplicated (`aether` + `aether_status` → `mcp__aether_status`); discover the exact set once with `omp -p --no-session --tools zzz </dev/null 2>&1 | grep 'Valid tools'` or rpc `get_state.dumpTools`. (2) **stdin must be closed** (`</dev/null`) or the run hangs waiting for EOF. (3) **text extraction**: with `--mode json` take the last assistant `message_end`; with plain `-p` (text mode) stdout is already just the answer, but then a quota error is only exit 1 + stderr. `write` is force-added to the tool set regardless; `--approval-mode yolo` is already the default here (`tools.approvalMode` → `yolo`). omp reads `.mcp.json` from the cwd just like `claude`, so the existing AETHER `.mcp.json` works unchanged provided the server key is `aether`.

## Open questions I could not resolve

1. **Your global omp settings** (`~/.omp/agent/config.yml` on the dev box): whether `retry.fallbackChains`, `modelRoles`, `enabledModels`, `disabledProviders` are set. Unknowable here; `omp config get retry.fallbackChains` on the dev box answers it in one line. The F.4 overlay makes the answer irrelevant for AETHER's runs.
2. **Which providers are actually logged in** on the dev box. The add-on's `docs/ENVIRONMENT.md` (2026-08-29) says `openai-codex, opencode-go, nanogpt, openrouter, zai, ollama` and explicitly **no direct `anthropic` credential** — if that is still true, "$0 subscription-backed Claude" via omp does not exist today; Claude traffic would go through nanogpt/openrouter (metered) or needs `omp auth-broker login anthropic` / `/login`. `omp usage --json` and `omp token anthropic --list` settle it.
3. **Real 429/usage-limit frame samples** for anthropic OAuth, openai-codex and gemini/antigravity — needs a logged-in box; capture with `omp -p --mode json --no-session … 2>stderr.txt | tee frames.ndjson` while a window is exhausted.
4. **Whether the model reliably calls `mcp__aether_*` / host tools in headless mode** (E.4) — omp presents MCP tools as `xd://` devices, not native tool definitions; behavior may vary per provider.
5. **The 10.1b spec and `scan_all.sh`/`enrich_all.sh` contents** — not in the repository clone; F.6 is based on the prompt's description.
6. **Decision #127 text** — not in the repo's DECISIONS files; I took "rejecting cross-provider fallback" at face value. Whether *same-provider account rotation* is acceptable under it is a judgment call for you (F.4 caveat 1).
7. **`write` force-add**: I did not locate which component adds `write` to every tool set (it survives `--no-extensions`); for strictly read-only passes an extension `tool_call` block (as the add-on's write guard does) or a wrapper that rejects `write` calls is the safe assumption.
8. **Behavior of `retry.maxDelayMs: 0`** (omp sleeping through multi-hour windows) in rpc mode — not exercised; the code path (`turn-recovery.ts:2214`) is clear but the frame cadence while sleeping is not.
9. **ACP mode** (`omp acp`) as an alternative to rpc for the Tauri app — not evaluated beyond noting it exists.
