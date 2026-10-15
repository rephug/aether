#!/usr/bin/env node
// Exercises the compiled enhance-prompt command against a stub `vscode`
// module and a local HTTP server standing in for the AETHER daemon.
// Run `npm run build` first; `npm run verify` does both.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const Module = require('node:module');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const distPath = path.resolve(__dirname, '..', 'dist', 'enhancePrompt.js');

function makeEnhanceResult(enhancedPrompt) {
  return {
    enhanced_prompt: enhancedPrompt,
    resolved_symbols: ['demo::symbol'],
    referenced_files: ['src/lib.rs'],
    rewrite_used: false,
    token_count: 42,
    warnings: [],
  };
}

function createVscodeStub(workspacePath, port) {
  const opened = [];
  const messages = [];
  const token = {
    isCancellationRequested: false,
    onCancellationRequested: () => ({ dispose() {} }),
  };
  const vscode = {
    ProgressLocation: { Notification: 15 },
    Selection: class Selection {},
    workspace: {
      workspaceFolders: [{ uri: { fsPath: workspacePath } }],
      getConfiguration: () => ({
        get: (key, fallback) => (key === 'daemonPort' ? port : fallback),
      }),
      openTextDocument: async ({ content }) => {
        opened.push(content);
        return { content };
      },
    },
    window: {
      activeTextEditor: undefined,
      showInputBox: async () => 'fix the auth bug in the login flow',
      withProgress: async (_options, task) => task({ report() {} }, token),
      showTextDocument: async () => undefined,
      showInformationMessage: (message) => messages.push(`info: ${message}`),
      showWarningMessage: (message) => messages.push(`warning: ${message}`),
      showErrorMessage: (message) => messages.push(`error: ${message}`),
    },
  };
  return { vscode, opened, messages };
}

function loadEnhanceModule(vscodeStub) {
  const originalLoad = Module._load;
  Module._load = function load(request, parent, isMain) {
    if (request === 'vscode') {
      return vscodeStub;
    }
    return originalLoad.call(this, request, parent, isMain);
  };
  try {
    delete require.cache[distPath];
    return require(distPath);
  } finally {
    Module._load = originalLoad;
  }
}

async function startFakeDaemon(daemonWorkspace) {
  const requests = [];
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', (chunk) => {
      body += chunk;
    });
    req.on('end', () => {
      requests.push({ method: req.method, url: req.url, body });
      res.setHeader('Content-Type', 'application/json');
      if (req.method === 'GET' && req.url === '/api/v1/daemon-status') {
        res.end(
          JSON.stringify({
            daemon: true,
            pid: 1,
            port: server.address().port,
            workspace: daemonWorkspace,
            uptime_seconds: 1,
          })
        );
        return;
      }
      if (req.method === 'POST' && req.url === '/api/enhance') {
        res.end(JSON.stringify(makeEnhanceResult(`daemon result for ${daemonWorkspace}`)));
        return;
      }
      res.statusCode = 404;
      res.end(JSON.stringify({ error: 'not_found', message: 'not found' }));
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, port: server.address().port, requests };
}

async function runEnhance(workspacePath, daemonWorkspace) {
  const daemon = await startFakeDaemon(daemonWorkspace);
  try {
    const stub = createVscodeStub(workspacePath, daemon.port);
    const { createEnhancePromptCommand } = loadEnhanceModule(stub.vscode);
    const cliRuns = [];
    const command = createEnhancePromptCommand(
      { extensionPath: path.join(workspacePath, 'vscode-extension') },
      {
        outputChannel: { appendLine() {} },
        ensureAetherdBinary: async () => 'aetherd',
        runAetherdProcess: async (_binary, args) => {
          cliRuns.push(args);
          return {
            exitCode: 0,
            stdout: JSON.stringify(makeEnhanceResult('cli result for local workspace')),
            stderr: '',
            cancelled: false,
          };
        },
      }
    );
    await command();
    return { ...stub, cliRuns, daemonRequests: daemon.requests };
  } finally {
    await new Promise((resolve) => daemon.server.close(resolve));
  }
}

function makeTempDir(t, label) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `aether-enhance-${label}-`));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test('daemon serving another workspace is ignored in favor of the CLI', async (t) => {
  assert.ok(fs.existsSync(distPath), 'missing dist/enhancePrompt.js (run npm run build first)');
  const localWorkspace = makeTempDir(t, 'local');
  const otherWorkspace = makeTempDir(t, 'other');

  const run = await runEnhance(localWorkspace, otherWorkspace);

  assert.equal(run.cliRuns.length, 1, `expected one CLI run, messages: ${run.messages}`);
  assert.deepEqual(run.cliRuns[0].slice(0, 2), ['--workspace', localWorkspace]);
  assert.deepEqual(run.opened, ['cli result for local workspace']);
  assert.ok(
    !run.daemonRequests.some((request) => request.url === '/api/enhance'),
    'enhance request must not be sent to a daemon serving another workspace'
  );
});

test('daemon serving the same workspace (via symlink) is used', async (t) => {
  assert.ok(fs.existsSync(distPath), 'missing dist/enhancePrompt.js (run npm run build first)');
  const realWorkspace = makeTempDir(t, 'real');
  const linkParent = makeTempDir(t, 'link');
  const linkedWorkspace = path.join(linkParent, 'workspace');
  // 'junction' needs no extra privileges on Windows (a 'dir' symlink needs
  // admin rights or Developer Mode); POSIX ignores the type argument.
  fs.symlinkSync(realWorkspace, linkedWorkspace, 'junction');

  const run = await runEnhance(linkedWorkspace, realWorkspace);

  assert.equal(run.cliRuns.length, 0, `expected no CLI run, messages: ${run.messages}`);
  assert.deepEqual(run.opened, [`daemon result for ${realWorkspace}`]);
});
