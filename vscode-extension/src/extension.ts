import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { spawn } from 'child_process';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Executable,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel | undefined;

type InferenceProvider = 'auto' | 'mock' | 'gemini' | 'qwen3_local';

interface AetherSettings {
  provider: InferenceProvider;
  model: string;
  endpoint: string;
  geminiApiKeyEnv: string;
}

interface ProviderQuickPickItem extends vscode.QuickPickItem {
  value: InferenceProvider;
}

const DEFAULT_MODEL = 'qwen3-embeddings-0.6B';
const DEFAULT_ENDPOINT = 'http://127.0.0.1:11434';
const DEFAULT_GEMINI_API_KEY_ENV = 'GEMINI_API_KEY';

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  outputChannel = vscode.window.createOutputChannel('AETHER');
  context.subscriptions.push(outputChannel);

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

  await startClient(context);
}

export async function deactivate(): Promise<void> {
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

  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
  if (!workspaceFolder) {
    vscode.window.showErrorMessage('AETHER: Open a workspace folder first.');
    return;
  }

  const repoRoot = path.resolve(context.extensionPath, '..');

  let binaryPath: string;
  try {
    binaryPath = await ensureAetherdBinary(repoRoot);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    vscode.window.showErrorMessage(`AETHER: Failed to prepare aetherd (${message})`);
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
  };

  client = new LanguageClient('aether', 'AETHER LSP', serverOptions, clientOptions);

  await client.start();

  outputChannel?.appendLine(`AETHER: Started LSP with ${binaryPath}`);
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

  return {
    provider,
    model,
    endpoint,
    geminiApiKeyEnv,
  };
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
