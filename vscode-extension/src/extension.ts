import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { ChildProcess, spawn } from 'child_process';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Executable,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;
let statusBarItem: vscode.StatusBarItem | undefined;
let workspaceMetaWatcher: vscode.FileSystemWatcher | undefined;
let statusRefreshTimer: NodeJS.Timeout | undefined;
let selectedWorkspaceFolder: vscode.WorkspaceFolder | undefined;
let lastSearchResults: SearchMatch[] = [];

type InferenceProvider = 'auto' | 'mock' | 'gemini' | 'qwen3_local';
type SearchMode = 'lexical' | 'semantic' | 'hybrid';

interface AetherSettings {
  provider: InferenceProvider;
  model: string;
  endpoint: string;
  geminiApiKeyEnv: string;
  searchMode: SearchMode;
}

interface ProviderQuickPickItem extends vscode.QuickPickItem {
  value: InferenceProvider;
}

interface SearchMatch {
  symbol_id: string;
  qualified_name: string;
  file_path: string;
  language: string;
  kind: string;
  semantic_score: number | null;
}

interface SearchEnvelope {
  mode_requested: string;
  mode_used: string;
  fallback_reason: string | null;
  matches: SearchMatch[];
}

interface SearchQuickPickItem extends vscode.QuickPickItem {
  match: SearchMatch;
}

interface ProcessRunResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  cancelled: boolean;
}

interface StatusModel {
  activeTasks: Set<string>;
  lastIndexActivityAt: number | undefined;
  startupUntil: number;
  staleObservationKeys: Set<string>;
  lastError: string | undefined;
}

const DEFAULT_MODEL = 'qwen3-embeddings-0.6B';
const DEFAULT_ENDPOINT = 'http://127.0.0.1:11434';
const DEFAULT_GEMINI_API_KEY_ENV = 'GEMINI_API_KEY';
const DEFAULT_SEARCH_MODE: SearchMode = 'lexical';

const INDEXING_ACTIVITY_WINDOW_MS = 5_000;
const STARTUP_INDEXING_WINDOW_MS = 8_000;
const STATUS_REFRESH_INTERVAL_MS = 1_000;
const STALE_WARNING_FRAGMENT = 'AETHER WARNING: SIR is stale';

const statusModel: StatusModel = {
  activeTasks: new Set<string>(),
  lastIndexActivityAt: undefined,
  startupUntil: 0,
  staleObservationKeys: new Set<string>(),
  lastError: undefined,
};

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  outputChannel = vscode.window.createOutputChannel('AETHER');
  context.subscriptions.push(outputChannel);

  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  statusBarItem.command = 'aether.searchSymbols';
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  resetWorkspaceSelectionAndWatcher(context);
  renderStatusBar();

  statusRefreshTimer = setInterval(() => {
    renderStatusBar();
  }, STATUS_REFRESH_INTERVAL_MS);
  context.subscriptions.push(
    new vscode.Disposable(() => {
      if (statusRefreshTimer) {
        clearInterval(statusRefreshTimer);
        statusRefreshTimer = undefined;
      }
    })
  );

  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      const previousWorkspace = selectedWorkspaceFolder?.uri.toString();
      resetWorkspaceSelectionAndWatcher(context);
      const nextWorkspace = selectedWorkspaceFolder?.uri.toString();
      if (client && previousWorkspace !== nextWorkspace) {
        void restartClient(context);
      }
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('aether.restartServer', async () => {
      await restartClient(context);
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('aether.selectInferenceProvider', async () => {
      await selectInferenceProvider();
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('aether.indexOnce', async () => {
      await runIndexOnce(context);
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('aether.searchSymbols', async () => {
      await runSearchSymbols(context);
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('aether.openSymbolResult', async () => {
      await openCachedSearchResult();
    })
  );

  await startClient(context);
}

export async function deactivate(): Promise<void> {
  if (workspaceMetaWatcher) {
    workspaceMetaWatcher.dispose();
    workspaceMetaWatcher = undefined;
  }

  if (statusRefreshTimer) {
    clearInterval(statusRefreshTimer);
    statusRefreshTimer = undefined;
  }

  await stopClient();
}

async function restartClient(context: vscode.ExtensionContext): Promise<void> {
  await stopClient();
  await startClient(context);
}

async function selectInferenceProvider(): Promise<void> {
  const config = vscode.workspace.getConfiguration('aether');
  const current = readSettings().provider;

  const options: ProviderQuickPickItem[] = [
    {
      label: 'auto',
      detail: 'Use Gemini when API key exists; otherwise use Mock.',
      value: 'auto',
    },
    {
      label: 'mock',
      detail: 'Deterministic local mock provider.',
      value: 'mock',
    },
    {
      label: 'gemini',
      detail: 'Gemini API using configured API key env var.',
      value: 'gemini',
    },
    {
      label: 'qwen3_local',
      detail: 'Local HTTP endpoint provider (no API key required).',
      value: 'qwen3_local',
    },
  ];

  const picked = await vscode.window.showQuickPick(options, {
    title: 'AETHER: Select Inference Provider',
    placeHolder: `Current: ${current}`,
  });

  if (!picked) {
    return;
  }

  await config.update('inferenceProvider', picked.value, vscode.ConfigurationTarget.Workspace);
  outputChannel?.appendLine(`AETHER: inferenceProvider set to ${picked.value}`);
}

async function startClient(context: vscode.ExtensionContext): Promise<void> {
  if (client) {
    return;
  }

  const workspaceFolder = getSelectedWorkspaceFolder();
  if (!workspaceFolder) {
    const message = 'AETHER: Open a workspace folder first.';
    vscode.window.showErrorMessage(message);
    setLastError(message);
    return;
  }

  selectedWorkspaceFolder = workspaceFolder;
  statusModel.startupUntil = Date.now() + STARTUP_INDEXING_WINDOW_MS;
  renderStatusBar();

  const repoRoot = path.resolve(context.extensionPath, '..');

  let binaryPath: string;
  try {
    binaryPath = await ensureAetherdBinary(repoRoot);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const wrapped = `Failed to prepare aetherd (${message})`;
    setLastError(wrapped);
    vscode.window.showErrorMessage(`AETHER: ${wrapped}`);
    statusModel.startupUntil = 0;
    renderStatusBar();
    return;
  }

  const settings = readSettings();
  const args = [
    '--',
    '--workspace',
    workspaceFolder.uri.fsPath,
    '--lsp',
    '--index',
    '--inference-provider',
    settings.provider,
    '--inference-model',
    settings.model,
    '--inference-endpoint',
    settings.endpoint,
    '--inference-api-key-env',
    settings.geminiApiKeyEnv,
  ];

  const executable: Executable = {
    command: binaryPath,
    args,
    options: {
      cwd: repoRoot,
      env: process.env,
    },
  };

  const serverOptions: ServerOptions = executable;

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: 'file', language: 'rust' },
      { scheme: 'file', language: 'typescript' },
      { scheme: 'file', language: 'typescriptreact' },
      { scheme: 'file', language: 'javascript' },
      { scheme: 'file', language: 'javascriptreact' },
    ],
    outputChannel,
    middleware: {
      provideHover: async (document, position, token, next) => {
        const hover = await next(document, position, token);
        observeHoverForStaleWarning(document, position, hover);
        return hover;
      },
    },
  };

  client = new LanguageClient('aether', 'AETHER LSP', serverOptions, clientOptions);

  try {
    await client.start();
    clearLastError();
    outputChannel?.appendLine(`AETHER: Started LSP with ${binaryPath}`);
  } catch (error) {
    client = undefined;
    const message = error instanceof Error ? error.message : String(error);
    const wrapped = `Failed to start language client (${message})`;
    setLastError(wrapped);
    vscode.window.showErrorMessage(`AETHER: ${wrapped}`);
  } finally {
    statusModel.startupUntil = 0;
    renderStatusBar();
  }
}

function observeHoverForStaleWarning(
  document: vscode.TextDocument,
  position: vscode.Position,
  hover: vscode.Hover | undefined | null
): void {
  if (!hover) {
    return;
  }

  const contentText = hoverContentsToText(hover.contents);
  if (!contentText.includes(STALE_WARNING_FRAGMENT)) {
    return;
  }

  const key = `${document.uri.toString()}:${position.line}:${position.character}`;
  if (!statusModel.staleObservationKeys.has(key)) {
    statusModel.staleObservationKeys.add(key);
    renderStatusBar();
  }
}

function hoverContentsToText(contents: vscode.Hover['contents']): string {
  if (Array.isArray(contents)) {
    return contents.map(markedStringToText).join('\n');
  }

  return markedStringToText(contents);
}

function markedStringToText(value: vscode.MarkdownString | vscode.MarkedString): string {
  if (typeof value === 'string') {
    return value;
  }

  if ('value' in value && typeof value.value === 'string') {
    return value.value;
  }

  return '';
}

async function runIndexOnce(context: vscode.ExtensionContext): Promise<void> {
  const workspaceFolder = getSelectedWorkspaceFolder();
  if (!workspaceFolder) {
    const message = 'Open a workspace folder before running index once.';
    setLastError(message);
    vscode.window.showErrorMessage(`AETHER: ${message}`);
    return;
  }

  const repoRoot = path.resolve(context.extensionPath, '..');

  let binaryPath: string;
  try {
    binaryPath = await ensureAetherdBinary(repoRoot);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const wrapped = `Failed to prepare aetherd (${message})`;
    setLastError(wrapped);
    vscode.window.showErrorMessage(`AETHER: ${wrapped}`);
    return;
  }

  const settings = readSettings();
  const args = [
    '--',
    '--workspace',
    workspaceFolder.uri.fsPath,
    '--index-once',
    '--inference-provider',
    settings.provider,
    '--inference-model',
    settings.model,
    '--inference-endpoint',
    settings.endpoint,
    '--inference-api-key-env',
    settings.geminiApiKeyEnv,
  ];

  markTaskStarted('index-once');
  try {
    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'AETHER: Index Once',
        cancellable: true,
      },
      async (_progress, token) => {
        const result = await runAetherdProcess(binaryPath, args, repoRoot, token);

        if (result.cancelled) {
          clearLastError();
          vscode.window.showWarningMessage('AETHER: Index once canceled.');
          return;
        }

        if (result.exitCode !== 0) {
          const message = `Index once failed with exit code ${result.exitCode ?? 'unknown'}`;
          setLastError(message);
          vscode.window.showErrorMessage(`AETHER: ${message}. See output channel for details.`);
          return;
        }

        recordIndexActivity();
        clearLastError();
        vscode.window.showInformationMessage('AETHER: Index once completed.');
      }
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const wrapped = `Index once failed (${message})`;
    setLastError(wrapped);
    vscode.window.showErrorMessage(`AETHER: ${wrapped}`);
  } finally {
    markTaskFinished('index-once');
  }
}

async function runSearchSymbols(context: vscode.ExtensionContext): Promise<void> {
  const workspaceFolder = getSelectedWorkspaceFolder();
  if (!workspaceFolder) {
    const message = 'Open a workspace folder before searching symbols.';
    setLastError(message);
    vscode.window.showErrorMessage(`AETHER: ${message}`);
    return;
  }

  const query = (await vscode.window.showInputBox({
    title: 'AETHER: Search Symbols',
    prompt: 'Enter a symbol query',
    ignoreFocusOut: true,
    value: '',
  }))?.trim();

  if (!query) {
    return;
  }

  const repoRoot = path.resolve(context.extensionPath, '..');

  let binaryPath: string;
  try {
    binaryPath = await ensureAetherdBinary(repoRoot);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const wrapped = `Failed to prepare aetherd (${message})`;
    setLastError(wrapped);
    vscode.window.showErrorMessage(`AETHER: ${wrapped}`);
    return;
  }

  const settings = readSettings();
  const args = [
    '--',
    '--workspace',
    workspaceFolder.uri.fsPath,
    '--search',
    query,
    '--search-mode',
    settings.searchMode,
    '--output',
    'json',
  ];

  let matchesToOpen: SearchMatch[] = [];
  markTaskStarted('search');
  try {
    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: 'AETHER: Search Symbols',
        cancellable: true,
      },
      async (_progress, token) => {
        const result = await runAetherdProcess(binaryPath, args, repoRoot, token);

        if (result.cancelled) {
          clearLastError();
          vscode.window.showWarningMessage('AETHER: Search canceled.');
          return;
        }

        if (result.exitCode !== 0) {
          const message = `Search failed with exit code ${result.exitCode ?? 'unknown'}`;
          setLastError(message);
          vscode.window.showErrorMessage(`AETHER: ${message}. See output channel for details.`);
          return;
        }

        const envelope = parseSearchEnvelope(result.stdout);
        lastSearchResults = envelope.matches;
        matchesToOpen = envelope.matches;

        if (envelope.fallback_reason) {
          outputChannel?.appendLine(`AETHER: search fallback reason: ${envelope.fallback_reason}`);
        }

        if (envelope.matches.length === 0) {
          clearLastError();
          vscode.window.showInformationMessage(
            `AETHER: No symbols found for "${query}" (${envelope.mode_used}).`
          );
          return;
        }

        clearLastError();
      }
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const wrapped = `Search failed (${message})`;
    setLastError(wrapped);
    vscode.window.showErrorMessage(`AETHER: ${wrapped}`);
    return;
  } finally {
    markTaskFinished('search');
  }

  if (matchesToOpen.length > 0) {
    await pickAndOpenSearchResult(
      matchesToOpen,
      workspaceFolder,
      `AETHER: Search Results for "${query}"`
    );
  }
}

async function openCachedSearchResult(): Promise<void> {
  const workspaceFolder = getSelectedWorkspaceFolder();
  if (!workspaceFolder) {
    const message = 'Open a workspace folder before opening search results.';
    setLastError(message);
    vscode.window.showErrorMessage(`AETHER: ${message}`);
    return;
  }

  if (lastSearchResults.length === 0) {
    vscode.window.showInformationMessage(
      'AETHER: No cached search results. Run "AETHER: Search Symbols" first.'
    );
    return;
  }

  await pickAndOpenSearchResult(lastSearchResults, workspaceFolder, 'AETHER: Open Symbol Result');
}

async function pickAndOpenSearchResult(
  matches: SearchMatch[],
  workspaceFolder: vscode.WorkspaceFolder,
  title: string
): Promise<void> {
  const items: SearchQuickPickItem[] = matches.map((match) => {
    const semanticScore =
      typeof match.semantic_score === 'number'
        ? ` | score ${match.semantic_score.toFixed(3)}`
        : '';
    return {
      label: match.qualified_name,
      description: match.file_path,
      detail: `${match.kind} | ${match.language}${semanticScore}`,
      match,
    };
  });

  const picked = await vscode.window.showQuickPick(items, {
    title,
    placeHolder: `Workspace: ${workspaceFolder.name}`,
    matchOnDescription: true,
    matchOnDetail: true,
  });

  if (!picked) {
    return;
  }

  await openSearchMatch(picked.match, workspaceFolder);
}

async function openSearchMatch(
  match: SearchMatch,
  workspaceFolder: vscode.WorkspaceFolder
): Promise<void> {
  const filePath = resolveWorkspacePath(workspaceFolder, match.file_path);
  if (!fs.existsSync(filePath)) {
    const message = `Search result file does not exist: ${filePath}`;
    setLastError(message);
    vscode.window.showErrorMessage(`AETHER: ${message}`);
    return;
  }

  const document = await vscode.workspace.openTextDocument(vscode.Uri.file(filePath));
  const editor = await vscode.window.showTextDocument(document, {
    preview: false,
  });

  const symbolName = extractLeafSymbolName(match.qualified_name);
  if (symbolName) {
    const range = findFirstSymbolRange(document, symbolName);
    if (range) {
      editor.selection = new vscode.Selection(range.start, range.end);
      editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
    }
  }

  clearLastError();
}

function resolveWorkspacePath(workspaceFolder: vscode.WorkspaceFolder, filePath: string): string {
  if (path.isAbsolute(filePath)) {
    return filePath;
  }

  return path.join(workspaceFolder.uri.fsPath, filePath);
}

function extractLeafSymbolName(qualifiedName: string): string | undefined {
  const segments = qualifiedName
    .split(/::|\.|#/)
    .map((part) => part.trim())
    .filter((part) => part.length > 0);

  const leaf = (segments[segments.length - 1] ?? qualifiedName).trim();
  const withoutSuffix = leaf.replace(/\(.*$/, '').replace(/<.*$/, '').trim();
  return withoutSuffix.length > 0 ? withoutSuffix : undefined;
}

function findFirstSymbolRange(
  document: vscode.TextDocument,
  symbolName: string
): vscode.Range | undefined {
  const escaped = escapeRegExp(symbolName);
  const regex = new RegExp(`\\b${escaped}\\b`);
  const text = document.getText();
  const matchIndex = text.search(regex);

  if (matchIndex < 0) {
    return undefined;
  }

  const start = document.positionAt(matchIndex);
  const end = document.positionAt(matchIndex + symbolName.length);
  return new vscode.Range(start, end);
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function parseSearchEnvelope(stdout: string): SearchEnvelope {
  const candidates = stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.startsWith('{') && line.endsWith('}'));

  const toTry = candidates.length > 0 ? [...candidates].reverse() : [stdout.trim()];

  let parsed: unknown;
  let lastError: unknown;
  for (const candidate of toTry) {
    if (!candidate) {
      continue;
    }

    try {
      parsed = JSON.parse(candidate);
      break;
    } catch (error) {
      lastError = error;
    }
  }

  if (!parsed || typeof parsed !== 'object') {
    throw new Error(`Search output was not valid JSON (${String(lastError ?? 'unknown parse error')})`);
  }

  const payload = parsed as {
    mode_requested?: unknown;
    mode_used?: unknown;
    fallback_reason?: unknown;
    matches?: unknown;
  };

  if (!Array.isArray(payload.matches)) {
    throw new Error('Search JSON missing matches array');
  }

  const matches = payload.matches.map((entry, index) => parseSearchMatch(entry, index));
  return {
    mode_requested: typeof payload.mode_requested === 'string' ? payload.mode_requested : 'unknown',
    mode_used: typeof payload.mode_used === 'string' ? payload.mode_used : 'unknown',
    fallback_reason: typeof payload.fallback_reason === 'string' ? payload.fallback_reason : null,
    matches,
  };
}

function parseSearchMatch(value: unknown, index: number): SearchMatch {
  if (!value || typeof value !== 'object') {
    throw new Error(`Search match at index ${index} is not an object`);
  }

  const entry = value as Record<string, unknown>;
  const requiredString = (field: string): string => {
    const fieldValue = entry[field];
    if (typeof fieldValue !== 'string' || fieldValue.trim().length === 0) {
      throw new Error(`Search match missing non-empty "${field}" at index ${index}`);
    }
    return fieldValue;
  };

  const semanticScoreRaw = entry.semantic_score;
  const semanticScore =
    typeof semanticScoreRaw === 'number' && Number.isFinite(semanticScoreRaw)
      ? semanticScoreRaw
      : null;

  return {
    symbol_id: requiredString('symbol_id'),
    qualified_name: requiredString('qualified_name'),
    file_path: requiredString('file_path'),
    language: requiredString('language'),
    kind: requiredString('kind'),
    semantic_score: semanticScore,
  };
}

function readSettings(): AetherSettings {
  const config = vscode.workspace.getConfiguration('aether');

  const provider = config.get<InferenceProvider>('inferenceProvider', 'auto');
  const model =
    config.get<string>('inferenceModel', DEFAULT_MODEL)?.trim() || DEFAULT_MODEL;
  const endpoint =
    config.get<string>('inferenceEndpoint', DEFAULT_ENDPOINT)?.trim() || DEFAULT_ENDPOINT;
  const geminiApiKeyEnv =
    config.get<string>('geminiApiKeyEnv', DEFAULT_GEMINI_API_KEY_ENV)?.trim() ||
    DEFAULT_GEMINI_API_KEY_ENV;
  const configuredSearchMode = config.get<string>('searchMode', DEFAULT_SEARCH_MODE);
  const searchMode = normalizeSearchMode(configuredSearchMode ?? DEFAULT_SEARCH_MODE);

  return {
    provider,
    model,
    endpoint,
    geminiApiKeyEnv,
    searchMode,
  };
}

function normalizeSearchMode(value: string): SearchMode {
  if (value === 'semantic' || value === 'hybrid' || value === 'lexical') {
    return value;
  }

  return DEFAULT_SEARCH_MODE;
}

async function stopClient(): Promise<void> {
  if (!client) {
    return;
  }

  const runningClient = client;
  client = undefined;
  await runningClient.stop();
}

async function ensureAetherdBinary(repoRoot: string): Promise<string> {
  const binaryName = process.platform === 'win32' ? 'aetherd.exe' : 'aetherd';
  const binaryPath = path.join(repoRoot, 'target', 'debug', binaryName);

  if (fs.existsSync(binaryPath)) {
    return binaryPath;
  }

  outputChannel?.appendLine('AETHER: aetherd binary not found. Running cargo build -p aetherd');
  await runCargoBuild(repoRoot);

  if (!fs.existsSync(binaryPath)) {
    throw new Error(`binary not found after build: ${binaryPath}`);
  }

  return binaryPath;
}

function runCargoBuild(repoRoot: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn('cargo', ['build', '-p', 'aetherd'], {
      cwd: repoRoot,
      env: process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    child.stdout.on('data', (chunk) => {
      outputChannel?.append(chunk.toString());
    });

    child.stderr.on('data', (chunk) => {
      outputChannel?.append(chunk.toString());
    });

    child.on('error', (error) => {
      reject(error);
    });

    child.on('close', (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`cargo build exited with code ${code ?? 'unknown'}`));
      }
    });
  });
}

function runAetherdProcess(
  binaryPath: string,
  args: string[],
  repoRoot: string,
  token: vscode.CancellationToken
): Promise<ProcessRunResult> {
  return new Promise((resolve, reject) => {
    let stdout = '';
    let stderr = '';
    let cancelled = false;

    const child = spawn(binaryPath, args, {
      cwd: repoRoot,
      env: process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    const cancelDisposable = token.onCancellationRequested(() => {
      cancelled = true;
      outputChannel?.appendLine('AETHER: command cancellation requested');
      terminateChildProcess(child);
    });

    child.stdout.on('data', (chunk) => {
      const text = chunk.toString();
      stdout += text;
      outputChannel?.append(text);
    });

    child.stderr.on('data', (chunk) => {
      const text = chunk.toString();
      stderr += text;
      outputChannel?.append(text);
    });

    child.on('error', (error) => {
      cancelDisposable.dispose();
      reject(error);
    });

    child.on('close', (code) => {
      cancelDisposable.dispose();
      resolve({
        exitCode: code,
        stdout,
        stderr,
        cancelled,
      });
    });
  });
}

function terminateChildProcess(child: ChildProcess): void {
  if (child.killed) {
    return;
  }

  try {
    child.kill();
  } catch {
    return;
  }

  setTimeout(() => {
    if (!child.killed) {
      try {
        child.kill('SIGKILL');
      } catch {
        // no-op
      }
    }
  }, 1500);
}

function getSelectedWorkspaceFolder(): vscode.WorkspaceFolder | undefined {
  return vscode.workspace.workspaceFolders?.[0];
}

function resetWorkspaceSelectionAndWatcher(context: vscode.ExtensionContext): void {
  if (workspaceMetaWatcher) {
    workspaceMetaWatcher.dispose();
    workspaceMetaWatcher = undefined;
  }

  selectedWorkspaceFolder = getSelectedWorkspaceFolder();

  if (!selectedWorkspaceFolder) {
    renderStatusBar();
    return;
  }

  const pattern = new vscode.RelativePattern(selectedWorkspaceFolder, '.aether/meta.sqlite');
  workspaceMetaWatcher = vscode.workspace.createFileSystemWatcher(pattern);

  context.subscriptions.push(workspaceMetaWatcher);
  context.subscriptions.push(
    workspaceMetaWatcher.onDidCreate(() => {
      recordIndexActivity();
    })
  );
  context.subscriptions.push(
    workspaceMetaWatcher.onDidChange(() => {
      recordIndexActivity();
    })
  );
  context.subscriptions.push(
    workspaceMetaWatcher.onDidDelete(() => {
      recordIndexActivity();
    })
  );

  renderStatusBar();
}

function recordIndexActivity(): void {
  statusModel.lastIndexActivityAt = Date.now();
  renderStatusBar();
}

function markTaskStarted(taskName: string): void {
  statusModel.activeTasks.add(taskName);
  renderStatusBar();
}

function markTaskFinished(taskName: string): void {
  statusModel.activeTasks.delete(taskName);
  renderStatusBar();
}

function setLastError(message: string): void {
  statusModel.lastError = message;
  outputChannel?.appendLine(`AETHER: error: ${message}`);
  renderStatusBar();
}

function clearLastError(): void {
  if (!statusModel.lastError) {
    return;
  }

  statusModel.lastError = undefined;
  renderStatusBar();
}

function renderStatusBar(): void {
  if (!statusBarItem) {
    return;
  }

  const workspaceFolder = selectedWorkspaceFolder ?? getSelectedWorkspaceFolder();
  if (!workspaceFolder) {
    statusBarItem.text = '$(circle-slash) AETHER: no workspace';
    statusBarItem.tooltip = 'Open a workspace folder to use AETHER commands.';
    return;
  }

  selectedWorkspaceFolder = workspaceFolder;

  const now = Date.now();
  const hasRecentIndexActivity =
    statusModel.lastIndexActivityAt !== undefined &&
    now - statusModel.lastIndexActivityAt <= INDEXING_ACTIVITY_WINDOW_MS;
  const indexing =
    statusModel.activeTasks.size > 0 ||
    hasRecentIndexActivity ||
    statusModel.startupUntil > now;

  const staleCount = statusModel.staleObservationKeys.size;
  const hasError = typeof statusModel.lastError === 'string' && statusModel.lastError.length > 0;

  let hint = '';
  if (hasError) {
    hint = ' (error)';
  } else if (staleCount > 0) {
    hint = ` (stale:${staleCount})`;
  }

  const baseState = indexing ? 'indexing' : 'idle';
  const icon = indexing
    ? '$(sync~spin)'
    : hasError
      ? '$(error)'
      : staleCount > 0
        ? '$(warning)'
        : '$(check)';

  statusBarItem.text = `${icon} AETHER: ${baseState}${hint}`;

  const tooltipLines = [
    `Workspace: ${workspaceFolder.name}`,
    `Folder: ${workspaceFolder.uri.fsPath}`,
    `Base state: ${baseState}`,
    `Active tasks: ${statusModel.activeTasks.size}`,
  ];

  if (statusModel.lastIndexActivityAt !== undefined) {
    tooltipLines.push(
      `Last index activity: ${new Date(statusModel.lastIndexActivityAt).toLocaleTimeString()}`
    );
  }

  if (staleCount > 0) {
    tooltipLines.push(`Observed stale hints: ${staleCount}`);
  }

  if (statusModel.lastError) {
    tooltipLines.push(`Last error: ${statusModel.lastError}`);
  }

  statusBarItem.tooltip = tooltipLines.join('\n');
}
