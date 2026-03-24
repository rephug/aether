import * as path from 'path';
import * as vscode from 'vscode';

const DEFAULT_DAEMON_PORT = 9730;
const DEFAULT_ENHANCE_BUDGET = 8000;
const ENHANCE_TIMEOUT_MS = 30_000;
const CANCELLED_ERROR = '__aether_enhance_cancelled__';

interface EnhanceProcessResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  cancelled: boolean;
}

interface EnhancePromptDependencies {
  outputChannel: vscode.OutputChannel | undefined;
  ensureAetherdBinary: (repoRoot: string) => Promise<string>;
  runAetherdProcess: (
    binaryPath: string,
    args: string[],
    repoRoot: string,
    token: vscode.CancellationToken
  ) => Promise<EnhanceProcessResult>;
}

interface EnhanceResult {
  enhanced_prompt: string;
  resolved_symbols: string[];
  referenced_files: string[];
  rewrite_used: boolean;
  token_count: number;
  warnings: string[];
}

interface ErrorBody {
  error?: string;
  message?: string;
}

export function createEnhancePromptCommand(
  context: vscode.ExtensionContext,
  deps: EnhancePromptDependencies
): () => Promise<void> {
  return async () => {
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    if (!workspaceFolder) {
      vscode.window.showErrorMessage(
        'AETHER: Open a workspace folder before enhancing prompts.'
      );
      return;
    }

    const editor = vscode.window.activeTextEditor;
    const selection =
      editor && !editor.selection.isEmpty ? new vscode.Selection(editor.selection.start, editor.selection.end) : undefined;
    const selectedText = selection ? editor?.document.getText(selection) : undefined;

    let promptText = selectedText && selectedText.trim().length > 0 ? selectedText : undefined;
    if (!promptText) {
      promptText = await vscode.window.showInputBox({
        title: 'AETHER: Enhance Prompt',
        prompt: 'Enter a coding prompt to enhance',
        placeHolder: 'e.g., fix the auth bug in the login flow',
        ignoreFocusOut: true,
      });
    }

    const normalizedPrompt = promptText?.trim();
    if (!normalizedPrompt) {
      return;
    }

    const config = vscode.workspace.getConfiguration('aether');
    const port = normalizePositiveNumber(
      config.get<number>('daemonPort', DEFAULT_DAEMON_PORT),
      DEFAULT_DAEMON_PORT
    );
    const budget = normalizePositiveNumber(
      config.get<number>('enhance.budget', DEFAULT_ENHANCE_BUDGET),
      DEFAULT_ENHANCE_BUDGET
    );
    const rewrite = config.get<boolean>('enhance.rewrite', false);
    const repoRoot = path.resolve(context.extensionPath, '..');

    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'AETHER: Enhancing prompt...',
        cancellable: true,
      },
      async (_progress, token) => {
        try {
          const result = await getEnhancementResult(
            normalizedPrompt,
            workspaceFolder.uri.fsPath,
            port,
            budget,
            rewrite,
            repoRoot,
            token,
            deps
          );
          await applyEnhancementResult(editor, selection, promptText ?? normalizedPrompt, result);

          for (const warning of result.warnings) {
            deps.outputChannel?.appendLine(`AETHER: enhance warning: ${warning}`);
          }

          const symbolCount = result.resolved_symbols.length;
          vscode.window.showInformationMessage(
            `AETHER: Enhanced with ${symbolCount} symbols resolved`
          );
        } catch (error) {
          if (isCancelledError(error)) {
            vscode.window.showWarningMessage('AETHER: Prompt enhancement canceled.');
            return;
          }

          const message = error instanceof Error ? error.message : String(error);
          vscode.window.showErrorMessage(
            `AETHER: Enhancement failed — ${message}. For daemon mode, run a dashboard-enabled aetherd (--features dashboard).`
          );
        }
      }
    );
  };
}

async function getEnhancementResult(
  promptText: string,
  workspacePath: string,
  port: number,
  budget: number,
  rewrite: boolean,
  repoRoot: string,
  token: vscode.CancellationToken,
  deps: EnhancePromptDependencies
): Promise<EnhanceResult> {
  try {
    return await requestEnhancementFromDaemon(promptText, port, budget, rewrite, token);
  } catch (error) {
    if (isCancelledError(error)) {
      throw error;
    }

    const message = error instanceof Error ? error.message : String(error);
    deps.outputChannel?.appendLine(`AETHER: daemon enhance failed, falling back to CLI (${message})`);
    return requestEnhancementFromCli(
      promptText,
      workspacePath,
      budget,
      rewrite,
      repoRoot,
      token,
      deps
    );
  }
}

async function requestEnhancementFromDaemon(
  promptText: string,
  port: number,
  budget: number,
  rewrite: boolean,
  token: vscode.CancellationToken
): Promise<EnhanceResult> {
  const controller = new AbortController();
  let cancelled = false;
  let timedOut = false;
  const cancelDisposable = token.onCancellationRequested(() => {
    cancelled = true;
    controller.abort();
  });
  const timeout = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, ENHANCE_TIMEOUT_MS);

  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/enhance`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        prompt: promptText,
        budget,
        rewrite,
      }),
      signal: controller.signal,
    });

    const body = await response.text();
    if (!response.ok) {
      throw new Error(parseErrorBody(body, `daemon returned ${response.status}`));
    }

    return parseEnhanceResult(body);
  } catch (error) {
    if (cancelled || token.isCancellationRequested) {
      throw new Error(CANCELLED_ERROR);
    }
    if (timedOut) {
      throw new Error(`daemon request timed out after ${ENHANCE_TIMEOUT_MS / 1000}s`);
    }
    throw error;
  } finally {
    clearTimeout(timeout);
    cancelDisposable.dispose();
  }
}

async function requestEnhancementFromCli(
  promptText: string,
  workspacePath: string,
  budget: number,
  rewrite: boolean,
  repoRoot: string,
  token: vscode.CancellationToken,
  deps: EnhancePromptDependencies
): Promise<EnhanceResult> {
  const binaryPath = await deps.ensureAetherdBinary(repoRoot);
  const args = [
    '--workspace',
    workspacePath,
    'enhance',
    promptText,
    '--output',
    'json',
    '--budget',
    String(budget),
  ];
  if (rewrite) {
    args.push('--rewrite');
  }

  const result = await deps.runAetherdProcess(binaryPath, args, repoRoot, token);
  if (result.cancelled || token.isCancellationRequested) {
    throw new Error(CANCELLED_ERROR);
  }

  if (result.exitCode !== 0) {
    const detail = result.stderr.trim() || result.stdout.trim() || 'unknown error';
    const exitCode = result.exitCode === null ? 'signal' : String(result.exitCode);
    throw new Error(`CLI fallback failed with exit code ${exitCode}: ${detail}`);
  }

  return parseEnhanceResult(result.stdout);
}

async function applyEnhancementResult(
  editor: vscode.TextEditor | undefined,
  selection: vscode.Selection | undefined,
  originalPrompt: string,
  result: EnhanceResult
): Promise<void> {
  const enhancedPrompt =
    result.enhanced_prompt.trim().length > 0 ? result.enhanced_prompt : originalPrompt;
  if (result.enhanced_prompt.trim().length === 0) {
    vscode.window.showWarningMessage(
      'AETHER: Enhancement returned empty output. Keeping the original prompt.'
    );
  }

  if (editor && selection) {
    const applied = await editor.edit((editBuilder) => {
      editBuilder.replace(selection, enhancedPrompt);
    });
    if (!applied) {
      throw new Error('failed to replace the selected prompt text');
    }
    return;
  }

  const document = await vscode.workspace.openTextDocument({
    content: enhancedPrompt,
    language: 'markdown',
  });
  await vscode.window.showTextDocument(document, {
    preview: false,
  });
}

function parseEnhanceResult(raw: string): EnhanceResult {
  const parsed = parseJsonPayload(raw);
  if (!parsed || typeof parsed !== 'object') {
    throw new Error('enhance output was not valid JSON');
  }

  const entry = parsed as Record<string, unknown>;
  return {
    enhanced_prompt: requiredString(entry, 'enhanced_prompt'),
    resolved_symbols: requiredStringArray(entry, 'resolved_symbols'),
    referenced_files: requiredStringArray(entry, 'referenced_files'),
    rewrite_used: requiredBoolean(entry, 'rewrite_used'),
    token_count: requiredNumber(entry, 'token_count'),
    warnings: requiredStringArray(entry, 'warnings'),
  };
}

function parseJsonPayload(raw: string): unknown {
  const trimmed = raw.trim();
  if (!trimmed) {
    throw new Error('enhance output was empty');
  }

  try {
    return JSON.parse(trimmed);
  } catch {
    const start = trimmed.indexOf('{');
    const end = trimmed.lastIndexOf('}');
    if (start >= 0 && end > start) {
      return JSON.parse(trimmed.slice(start, end + 1));
    }
    throw new Error('enhance output was not valid JSON');
  }
}

function parseErrorBody(raw: string, fallback: string): string {
  try {
    const parsed = JSON.parse(raw) as ErrorBody;
    if (typeof parsed.message === 'string' && parsed.message.trim().length > 0) {
      return parsed.message;
    }
  } catch {
    // ignored
  }

  const text = raw.trim();
  if (!text) {
    return fallback;
  }
  return `${fallback}: ${text}`;
}

function requiredString(entry: Record<string, unknown>, field: string): string {
  const value = entry[field];
  if (typeof value !== 'string') {
    throw new Error(`enhance JSON missing "${field}"`);
  }
  return value;
}

function requiredBoolean(entry: Record<string, unknown>, field: string): boolean {
  const value = entry[field];
  if (typeof value !== 'boolean') {
    throw new Error(`enhance JSON missing "${field}"`);
  }
  return value;
}

function requiredNumber(entry: Record<string, unknown>, field: string): number {
  const value = entry[field];
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new Error(`enhance JSON missing "${field}"`);
  }
  return value;
}

function requiredStringArray(entry: Record<string, unknown>, field: string): string[] {
  const value = entry[field];
  if (!Array.isArray(value) || value.some((item) => typeof item !== 'string')) {
    throw new Error(`enhance JSON missing "${field}"`);
  }
  return value as string[];
}

function normalizePositiveNumber(value: number | undefined, fallback: number): number {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    return fallback;
  }
  return Math.floor(value);
}

function isCancelledError(error: unknown): boolean {
  return error instanceof Error && error.message === CANCELLED_ERROR;
}
