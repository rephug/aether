# Phase CC.7 — VS Code Prompt Enhancer Command

**Phase:** CC — Claude Code Integration
**Prerequisites:** CC.6 (core enhance logic + CLI + MCP)
**Estimated Claude Code Runs:** 1
**Decision:** #109 — Daemon HTTP endpoint for VS Code enhancement

---

## Purpose

Add a VS Code command and keybinding that enhances the user's current input with AETHER codebase intelligence — directly in the editor, before submitting to any AI chat panel.

This is the premium UX surface for prompt enhancement. The user types a vague prompt into any chat input (Copilot Chat, Claude Code panel, Cursor chat, or the AETHER command palette), presses a keybinding, and the prompt is replaced with an enriched version.

---

## Decision #109: Daemon HTTP endpoint for VS Code enhancement

**Context:** The VS Code extension needs to call the enhancement logic. Three options: (A) shell out to `aether enhance` CLI, (B) LSP custom request, (C) daemon HTTP endpoint.

**Decision:** Daemon HTTP endpoint on the existing dashboard port.

**Rationale:**
- The dashboard already runs an HTTP server (port 9730) with HTMX routes
- Adding a `/api/enhance` JSON endpoint is trivial — same `SharedState`, same store access
- LSP custom requests are awkward for this (LSP is document-oriented, not prompt-oriented)
- Shelling out to CLI works but is slower (process spawn + store open overhead)
- The HTTP approach also enables future integrations (browser extensions, Raycast, etc.)

---

## Architecture

### Daemon Side: HTTP Endpoint

Add to the dashboard HTTP server (feature-gated behind `dashboard`):

```
POST /api/enhance
Content-Type: application/json

{
    "prompt": "fix the auth bug in the login flow",
    "budget": 8000,
    "rewrite": false
}
```

Response:

```json
{
    "enhanced_prompt": "## Enhanced Prompt\n\nfix the auth bug...\n\n## Relevant Context\n...",
    "resolved_symbols": ["AuthService::login", "TokenValidator::validate"],
    "referenced_files": ["src/auth/login.rs", "src/auth/token.rs"],
    "rewrite_used": false,
    "token_count": 3200,
    "warnings": []
}
```

The endpoint reuses the exact same `enhance_prompt_core()` function from CC.6. The dashboard route is a thin HTTP wrapper.

### Extension Side: Command + Keybinding

**New VS Code command:** `aether.enhancePrompt`

Behavior:
1. Read the current text from the active editor's selection (or entire line if no selection)
2. If no text, prompt user via `vscode.window.showInputBox()`
3. Show a progress indicator ("Enhancing prompt...")
4. POST to `http://localhost:{dashboard_port}/api/enhance`
5. Replace the selection (or insert at cursor) with the enhanced prompt
6. Show a brief notification with resolved symbol count

**Keybinding:** `Ctrl+Shift+E` (configurable, avoids conflicts with common VS Code bindings)

**Alternative activation:** Command palette → `AETHER: Enhance Prompt`

### Fallback Behavior

If the daemon is not running or the dashboard feature is disabled:
1. Fall back to CLI: `aether enhance "..." --output json`
2. Parse JSON response, replace selection
3. If CLI also fails, show error: "AETHER daemon not running. Start with `aetherd --workspace .`"

---

## New/Modified Files

### Daemon (Rust)

```
crates/aether-dashboard/src/api.rs          # New: JSON API routes (enhance endpoint)
crates/aether-dashboard/src/lib.rs          # Modified: mount /api routes
```

### Extension (TypeScript)

```
vscode-extension/src/enhancePrompt.ts       # New: enhance command implementation
vscode-extension/src/extension.ts           # Modified: register command
vscode-extension/package.json               # Modified: command + keybinding declarations
```

---

## VS Code Extension Changes

### package.json additions

```json
{
    "contributes": {
        "commands": [
            {
                "command": "aether.enhancePrompt",
                "title": "AETHER: Enhance Prompt",
                "category": "AETHER"
            }
        ],
        "keybindings": [
            {
                "command": "aether.enhancePrompt",
                "key": "ctrl+shift+e",
                "mac": "cmd+shift+e",
                "when": "editorTextFocus"
            }
        ],
        "configuration": {
            "properties": {
                "aether.enhance.budget": {
                    "type": "number",
                    "default": 8000,
                    "description": "Token budget for prompt enhancement context"
                },
                "aether.enhance.rewrite": {
                    "type": "boolean",
                    "default": false,
                    "description": "Use LLM to rewrite enhanced prompts into natural prose"
                },
                "aether.daemonPort": {
                    "type": "number",
                    "default": 9730,
                    "description": "Port for AETHER daemon HTTP API"
                }
            }
        }
    }
}
```

### enhancePrompt.ts

```typescript
import * as vscode from 'vscode';

export async function enhancePrompt() {
    const editor = vscode.window.activeTextEditor;
    
    // Get prompt text from selection or input box
    let promptText: string | undefined;
    
    if (editor && !editor.selection.isEmpty) {
        promptText = editor.document.getText(editor.selection);
    } else {
        promptText = await vscode.window.showInputBox({
            prompt: 'Enter a coding prompt to enhance',
            placeHolder: 'e.g., fix the auth bug in the login flow',
        });
    }
    
    if (!promptText) return;
    
    const config = vscode.workspace.getConfiguration('aether');
    const port = config.get<number>('daemonPort', 9730);
    const budget = config.get<number>('enhance.budget', 8000);
    const rewrite = config.get<boolean>('enhance.rewrite', false);
    
    await vscode.window.withProgress(
        {
            location: vscode.ProgressLocation.Notification,
            title: 'AETHER: Enhancing prompt...',
            cancellable: true,
        },
        async (progress, token) => {
            try {
                const response = await fetch(`http://localhost:${port}/api/enhance`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ prompt: promptText, budget, rewrite }),
                    signal: token.isCancellationRequested ? AbortSignal.abort() : undefined,
                });
                
                if (!response.ok) {
                    throw new Error(`Daemon returned ${response.status}`);
                }
                
                const result = await response.json();
                
                if (editor && !editor.selection.isEmpty) {
                    // Replace selection with enhanced prompt
                    await editor.edit(editBuilder => {
                        editBuilder.replace(editor.selection, result.enhanced_prompt);
                    });
                } else {
                    // Open in new untitled document
                    const doc = await vscode.workspace.openTextDocument({
                        content: result.enhanced_prompt,
                        language: 'markdown',
                    });
                    await vscode.window.showTextDocument(doc);
                }
                
                const symbolCount = result.resolved_symbols?.length ?? 0;
                vscode.window.showInformationMessage(
                    `AETHER: Enhanced with ${symbolCount} symbols resolved`
                );
                
            } catch (err: any) {
                // Fallback: try CLI
                // ... CLI fallback implementation ...
                vscode.window.showErrorMessage(
                    `AETHER: Enhancement failed — ${err.message}. Is the daemon running?`
                );
            }
        }
    );
}
```

---

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| Daemon not running | Fall back to CLI, then show error with start instructions |
| Dashboard feature disabled in build | CLI fallback only |
| No text selected, no input entered | Do nothing (user cancelled) |
| Enhancement returns empty/error | Show original prompt + warning toast |
| Keybinding conflicts | User can rebind in VS Code settings |
| Very slow enhancement (>10s) | Cancellable progress bar, timeout at 30s |
| Extension used in Cursor/Windsurf | Works identically (VS Code API compatible) |

---

## Pass Criteria

1. `POST /api/enhance` returns structured JSON response
2. `aether.enhancePrompt` command appears in command palette
3. `Ctrl+Shift+E` triggers enhancement when editor has focus
4. Selected text is replaced with enhanced prompt
5. No selection → input box → new document with result
6. Progress notification shown during enhancement
7. Error message shown when daemon is unreachable
8. VS Code extension builds: `cd vscode-extension && npm run build`
9. Rust: `cargo fmt --all --check` and `cargo clippy -p aether-dashboard -- -D warnings` pass
10. `cargo test -p aether-dashboard` passes

---

## Commit

**PR title:** `feat(vscode): prompt enhancer command with daemon HTTP API`

**PR body:**
```
Stage CC.7 of the Claude Code Integration phase.

Adds in-editor prompt enhancement via VS Code command:
- New daemon HTTP endpoint: POST /api/enhance (on dashboard port 9730)
- VS Code command: AETHER: Enhance Prompt (Ctrl+Shift+E)
- Replaces selected text with AETHER-enriched prompt
- Falls back to CLI when daemon unavailable
- Works in VS Code, Cursor, Windsurf, and any VS Code fork
- Configurable token budget and LLM rewrite mode

Reuses CC.6 core enhancement logic via the existing dashboard
HTTP server infrastructure.
```
