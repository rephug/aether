/* Offline regression tests for multi-root client handling in src/extension.ts: the
   extension runs against a mocked vscode API and language client, so no server starts. */
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const crypto = require('crypto');
const assert = require('assert/strict');
const { EventEmitter } = require('events');
const ts = require('../node_modules/typescript');

const outDir = process.argv[2] ? path.resolve(process.argv[2]) : fs.mkdtempSync(path.join(require('os').tmpdir(), 'aether-multiroot-regression-'));
fs.mkdirSync(outDir, { recursive: true });
const sourcePath = path.resolve(__dirname, '../src/extension.ts');
const extensionPath = path.resolve(sourcePath, '../..');
const manifestPath = path.join(extensionPath, 'package.json');
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const source = fs.readFileSync(sourcePath, 'utf8');
const sha256 = (text) => crypto.createHash('sha256').update(text).digest('hex');
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
  fileName: sourcePath,
  reportDiagnostics: true,
});
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, 'extension.exact-transpiled.cjs'), compiled.outputText);
const workspaceAPath = path.join(outDir, 'fixtures', 'workspace-a');
const workspaceBPath = path.join(outDir, 'fixtures', 'workspace-b');
fs.mkdirSync(workspaceAPath, { recursive: true });
fs.mkdirSync(workspaceBPath, { recursive: true });
const sourceA = 'export function combine(a: number, b: number): number {\n  return a + b;\n}\n';
const sourceB = 'export function combine(a: number, b: number): number {\n  return a - b;\n}\n';
fs.writeFileSync(path.join(workspaceAPath, 'math.ts'), sourceA);
fs.writeFileSync(path.join(workspaceBPath, 'math.ts'), sourceB);

class Disposable {
  constructor(callback = () => {}) { this.callback = callback; this.disposed = false; }
  dispose() { this.disposed = true; this.callback(); }
}
function event() {
  const listeners = [];
  return {
    listeners,
    register(callback) { listeners.push(callback); return new Disposable(() => { const i = listeners.indexOf(callback); if (i >= 0) listeners.splice(i, 1); }); },
    emit(value) { return listeners.slice().map((callback) => callback(value)); },
  };
}
function uri(filePath) {
  return { fsPath: filePath, scheme: 'file', toString: () => 'file:///' + filePath.replace(/\\/g, '/').replace(/^\//, '') };
}
function folder(filePath, name, index) { return { uri: uri(filePath), name, index }; }
const workspaceA = folder(workspaceAPath, 'A', 0);
const workspaceB = folder(workspaceBPath, 'B', 1);
function document(filePath, text) {
  return {
    uri: uri(filePath), languageId: 'typescript', version: 1, isDirty: false,
    text, getText() { return this.text; },
    positionAt(offset) { const prefix = this.text.slice(0, offset).split('\n'); return { line: prefix.length - 1, character: prefix.at(-1).length }; },
    edit(text, dirty = true) { this.text = text; this.version++; this.isDirty = dirty; },
  };
}
function token(cancelled = false) {
  const cancelledEvent = event();
  return { isCancellationRequested: cancelled, onCancellationRequested: cancelledEvent.register.bind(cancelledEvent), cancel() { this.isCancellationRequested = true; cancelledEvent.emit(); } };
}
function fixtureHover(doc) {
  const text = doc.getText();
  assert.match(text, /export function combine\(a: number, b: number\)/);
  const operation = text.includes('return a + b;') ? 'sum' : text.includes('return a - b;') ? 'difference' : text.includes('return a * b;') ? 'product' : 'unknown';
  const index = text.indexOf('combine');
  return {
    contents: [{ value: '**combine** returns the ' + operation + ' of numeric inputs a and b.', isTrusted: false, supportHtml: false }],
    range: { start: doc.positionAt(index), end: doc.positionAt(index + 'combine'.length) },
  };
}

function environment(options = {}) {
  const state = { clients: [], spawns: [], outputs: [], errors: [], warnings: [], infos: [], watchers: [], intervals: [], commands: new Map(), now: 1700000000000, registeredEvents: {} };
  const folderEvent = event();
  const docEvent = event();
  const renameEvent = event();
  const deleteEvent = event();
  const status = { text: '', tooltip: '', show() {}, dispose() {} };
  const selected = options.workspaceFolders === undefined ? [workspaceA] : options.workspaceFolders;
  const settings = { inferenceProvider: 'mock', inferenceModel: 'fixture-model', ...options.settings };
  const vscode = {
    Disposable,
    StatusBarAlignment: { Left: 1 },
    ConfigurationTarget: { Workspace: 2 },
    ProgressLocation: { Notification: 15 },
    RelativePattern: class { constructor(base, pattern) { this.base = base; this.pattern = pattern; } },
    Uri: { file: uri },
    workspace: {
      workspaceFolders: selected,
      getWorkspaceFolder(resource) { return (this.workspaceFolders || []).filter(folder => { const rel=path.relative(folder.uri.fsPath,resource.fsPath);return rel==='' || (rel!=='..' && !rel.startsWith('..'+path.sep) && !path.isAbsolute(rel)); }).sort((a,b)=>b.uri.fsPath.length-a.uri.fsPath.length)[0]; },
      getConfiguration(section) {
        assert.equal(section, 'aether');
        return { get(key, fallback) { return Object.hasOwn(settings, key) ? settings[key] : (manifest.contributes.configuration.properties['aether.' + key]?.default ?? fallback); }, async update(key, value) { settings[key] = value; } };
      },
      onDidChangeWorkspaceFolders(callback) { state.registeredEvents.workspaceFolders = (state.registeredEvents.workspaceFolders || 0) + 1; return folderEvent.register(callback); },
      onDidChangeTextDocument(callback) { state.registeredEvents.textDocument = (state.registeredEvents.textDocument || 0) + 1; return docEvent.register(callback); },
      onDidRenameFiles(callback) { state.registeredEvents.rename = (state.registeredEvents.rename || 0) + 1; return renameEvent.register(callback); },
      onDidDeleteFiles(callback) { state.registeredEvents.delete = (state.registeredEvents.delete || 0) + 1; return deleteEvent.register(callback); },
      createFileSystemWatcher(pattern) {
        const create = event(); const change = event(); const del = event();
        const watcher = { pattern, create, change, del, onDidCreate: create.register.bind(create), onDidChange: change.register.bind(change), onDidDelete: del.register.bind(del), dispose() { this.disposed = true; } };
        state.watchers.push(watcher); return watcher;
      },
    },
    window: {
      createOutputChannel(name) { assert.equal(name, 'AETHER'); return { append(value) { state.outputs.push(value); }, appendLine(value) { state.outputs.push(value); }, dispose() {} }; },
      createStatusBarItem() { return status; },
      showErrorMessage(message) { state.errors.push(message); return Promise.resolve(undefined); },
      showWarningMessage(message) { state.warnings.push(message); return Promise.resolve(undefined); },
      showInformationMessage(message) { state.infos.push(message); return Promise.resolve(undefined); },
      showQuickPick() { return Promise.resolve(undefined); },
      showInputBox() { return Promise.resolve(undefined); },
    },
    commands: { registerCommand(id, callback) { state.commands.set(id, callback); return new Disposable(); } },
  };
  class LanguageClient {
    constructor(id, name, serverOptions, clientOptions) { Object.assign(this, { id, name, serverOptions, clientOptions, starts: 0, stops: 0 }); state.clients.push(this); }
    async start() {
      this.starts++;
      this.startFailed = false;
      try {
        if (options.onStart) await options.onStart(this);
        if (options.startError) throw options.startError;
      } catch (error) { this.startFailed = true; throw error; }
    }
    async stop() {
      this.stops++;
      // Like vscode-languageclient 9: a client whose start failed refuses to stop.
      if (this.startFailed) throw new Error("Client is not running and can't be stopped");
      if (options.onStop) await options.onStop(this);
    }
  }
  class ControlledDate extends Date {
    constructor(...args) { super(...(args.length ? args : [state.now])); }
    static now() { return state.now; }
  }
  const fakeFs = { existsSync(filePath) { state.lastExistsPath = filePath; return options.binaryExists !== false; } };
  const fakeProcess = { platform: 'win32', env: options.environment ?? {} };
  const fakeSpawn = (command, args, spawnOptions) => {
    state.spawns.push({ command, args, options: spawnOptions });
    const child = new EventEmitter(); child.stdout = new EventEmitter(); child.stderr = new EventEmitter(); child.killed = false;
    child.kill = () => { child.killed = true; };
    queueMicrotask(() => child.emit('error', options.spawnError ?? new Error('OFFLINE_FIXTURE: cargo unavailable')));
    return child;
  };
  const exports = {};
  const sandbox = {
    exports, module: { exports }, process: fakeProcess, Date: ControlledDate,
    setInterval(callback, milliseconds) { const item = { callback, milliseconds, cleared: false }; state.intervals.push(item); return item; },
    clearInterval(item) { item.cleared = true; },
    setTimeout(callback) { return { callback }; }, clearTimeout() {},
    require(name) {
      if (name === 'vscode') return vscode;
      if (name === 'path') return path;
      if (name === 'fs') return fakeFs;
      if (name === 'child_process') return { spawn: fakeSpawn };
      if (name === 'vscode-languageclient/node') return { LanguageClient };
      if (name === './enhancePrompt') return { createEnhancePromptCommand() { state.enhanceFactoryCalls = (state.enhanceFactoryCalls || 0) + 1; return () => { throw new Error('Enhance command deliberately not executed'); }; } };
      if (name === './symbolNavigation') return { revealSymbolDefinition: async () => true };
      throw new Error('Unexpected module import: ' + name);
    },
  };
  vm.runInNewContext(compiled.outputText + '\nexports.__drain=()=>clientOperations;\n', sandbox, { filename: sourcePath + '.transpiled', timeout: 5000 });
  const context = { extensionPath, subscriptions: [] };
  return {
    state, status, vscode, context, exports,
    activate: () => exports.activate(context),
    hover: (...args) => state.clients.at(-1).clientOptions.middleware.provideHover(...args),
    async advance(milliseconds) { state.now += milliseconds; state.intervals.filter((i) => !i.cleared).forEach((i) => i.callback()); await settle(); },
    async folders(folders) { const previous=vscode.workspace.workspaceFolders||[];vscode.workspace.workspaceFolders=folders;folderEvent.emit({added:folders.filter(f=>!previous.some(p=>p.uri.toString()===f.uri.toString())),removed:previous.filter(p=>!folders.some(f=>p.uri.toString()===f.uri.toString()))});await exports.__drain();await settle(); },
    docChange: (doc) => docEvent.emit({ document: doc }),
    rename: (oldUri, newUri) => renameEvent.emit({ files: [{ oldUri, newUri }] }),
    async dispose() { await exports.deactivate(); context.subscriptions.forEach((entry) => entry.dispose()); },
  };
}
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); }
const results = [];
const TEST_TIMEOUT_MS = 5000;
async function test(name, callback, requirement = '') {
  // A promise the extension never settles fails the case instead of hanging the run.
  let timer;
  const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`timed out after ${TEST_TIMEOUT_MS} ms`)), TEST_TIMEOUT_MS); });
  try { const details = await Promise.race([callback(), timeout]); results.push({ name, status: 'PASS', requirement, details: details ?? {} }); }
  catch (error) { results.push({ name, status: 'FAIL', requirement, error: error.message }); }
  finally { clearTimeout(timer); }
}
const makeDoc = (folder = workspaceA, text = sourceA) => document(path.join(folder.uri.fsPath, 'math.ts'), text);
const position = { line: 0, character: 18 };
const stale = { contents: [{ value: '**combine**\n\nAETHER WARNING: SIR is stale', isTrusted: false }], range: { start: { line: 0, character: 16 }, end: { line: 0, character: 23 } } };

async function main() {
  assert.equal(compiled.diagnostics.filter((diagnostic) => diagnostic.category === ts.DiagnosticCategory.Error).length, 0);
  await test('activation configures exact AETHER identity, selected root, mock provider, languages, and registered commands', async () => {
    const env = environment(); await env.activate();
    assert.equal(env.state.clients.length, 1);
    const client = env.state.clients[0];
    assert.equal(client.id, 'aether:'+workspaceA.uri.toString()); assert.equal(client.name, 'AETHER LSP');
    assert.equal(client.serverOptions.command, path.resolve(extensionPath, '../target/debug/aetherd.exe'));
    assert.deepEqual(Array.from(client.serverOptions.args), ['--', '--workspace', workspaceAPath, '--lsp', '--index', '--inference-provider', 'mock', '--inference-model', 'fixture-model', '--inference-endpoint', 'http://127.0.0.1:11434', '--inference-api-key-env', 'GEMINI_API_KEY']);
    assert.deepEqual(Array.from(client.clientOptions.documentSelector, (value) => [value.scheme, value.language]), [['file', 'rust'], ['file', 'typescript'], ['file', 'typescriptreact'], ['file', 'javascript'], ['file', 'javascriptreact']]);
    assert.equal(env.state.commands.size, 6); assert.equal(env.state.spawns.length, 0); assert.equal(env.state.errors.length, 0);
    await env.dispose(); return { languageClientId: client.id, registeredCommands: [...env.state.commands.keys()], actualChildProcesses: 0 };
  });
  await test('hover preserves document, position, cancellation token, result, symbol range, and fixture explanation', async () => {
    const env = environment(); await env.activate(); const doc = makeDoc(); const tok = token(); const expected = fixtureHover(doc); let calls = 0;
    const actual = await env.hover(doc, position, tok, (nextDoc, nextPosition, nextToken) => { calls++; assert.strictEqual(nextDoc, doc); assert.strictEqual(nextPosition, position); assert.strictEqual(nextToken, tok); return Promise.resolve(expected); });
    assert.equal(calls, 1); assert.strictEqual(actual, expected); assert.equal(actual.contents[0].value, '**combine** returns the sum of numeric inputs a and b.');
    assert.deepEqual(actual.range, { start: { line: 0, character: 16 }, end: { line: 0, character: 23 } });
    assert.equal(env.state.spawns.length, 0); await env.dispose(); return { scope: 'fixture-backed response forwarding; source semantics supplied by deterministic next mock' };
  });
  for (const value of [undefined, null]) await test('hover preserves ' + String(value) + ' no-symbol result', async () => {
    const env = environment(); await env.activate(); assert.strictEqual(await env.hover(makeDoc(), position, token(), async () => value), value); assert.match(env.status.text, /idle$/); await env.dispose();
  });
  await test('hover propagates backend error identity without spawn, swallowing, or stale status', async () => {
    const env = environment(); await env.activate(); const error = new Error('fixture backend disconnected');
    await assert.rejects(env.hover(makeDoc(), position, token(), async () => { throw error; }), (actual) => actual === error);
    assert.equal(env.state.spawns.length, 0); assert.equal(env.state.errors.length, 0); assert.match(env.status.text, /idle$/); await env.dispose();
  });
  await test('hover propagates cancellation rejection and forwards pre-cancelled token unchanged', async () => {
    const env = environment(); await env.activate(); const tok = token(true); const error = new Error('fixture cancellation'); error.name = 'CancellationError';
    await assert.rejects(env.hover(makeDoc(), position, tok, async (_doc, _pos, actualToken) => { assert.strictEqual(actualToken, tok); assert.equal(actualToken.isCancellationRequested, true); throw error; }), (actual) => actual === error);
    assert.equal(env.state.spawns.length, 0); await env.dispose();
  });
  await test('hover forwards cancellation fired during an in-flight request', async () => {
    const env = environment(); await env.activate(); const tok = token(); const error = new Error('fixture in-flight cancellation'); let subscription;
    const request = env.hover(makeDoc(), position, tok, (_doc, _pos, actualToken) => new Promise((_resolve, reject) => { subscription = actualToken.onCancellationRequested(() => reject(error)); }));
    tok.cancel(); await assert.rejects(request, (actual) => actual === error); assert.equal(env.state.spawns.length, 0); subscription.dispose(); await env.dispose();
  });
  await test('hover preserves untrusted markup and does not invoke commands or processes', async () => {
    const env = environment(); await env.activate(); const content = { value: '[Do not run](command:workbench.action.closeWindow)\n<img src="x" onerror="alert(1)">\n<script>alert(1)</script>', isTrusted: false, supportHtml: false }; const expected = { contents: [content] };
    const actual = await env.hover(makeDoc(), position, token(), async () => expected);
    assert.strictEqual(actual.contents[0], content); assert.equal(actual.contents[0].isTrusted, false); assert.equal(actual.contents[0].supportHtml, false); assert.equal(env.state.spawns.length, 0); await env.dispose();
    return { limitation: 'No VS Code renderer exercised; incoming trusted MarkdownString is also returned unchanged.' };
  });
  await test('hover reevaluates next after unsaved source edits and has no extension-side result cache', async () => {
    const env = environment(); await env.activate(); const doc = makeDoc(); const tok = token(); let calls = 0;
    const next = async (currentDoc) => { assert.strictEqual(currentDoc, doc); calls++; return fixtureHover(currentDoc); };
    const before = await env.hover(doc, position, tok, next); doc.edit(sourceA.replace('a + b', 'a * b')); env.docChange(doc);
    const after = await env.hover(doc, position, tok, next); assert.equal(calls, 2); assert.equal(doc.isDirty, true); assert.equal(doc.version, 2); assert.match(before.contents[0].value, /sum/); assert.match(after.contents[0].value, /product/); assert.notStrictEqual(before, after); assert.equal(env.state.spawns.length, 0); await env.dispose();
    return { scope: 'Extension passes current document to next; backend handling of unsaved didChange is outside this mocked client.' };
  });
  await test('hover after rename receives the renamed document URI, without extension-side cached hover', async () => {
    const env = environment(); await env.activate(); const beforeDoc = makeDoc(); await env.hover(beforeDoc, position, token(), async () => fixtureHover(beforeDoc)); const afterDoc = document(path.join(workspaceAPath, 'renamed.ts'), sourceA); env.rename(beforeDoc.uri, afterDoc.uri);
    const after = await env.hover(afterDoc, position, token(), async (doc) => { assert.strictEqual(doc, afterDoc); assert.match(doc.uri.fsPath, /renamed\.ts$/); return fixtureHover(doc); }); assert.match(after.contents[0].value, /sum/); await env.dispose();
    return { scope: 'URI forwarding only; server index rename/move synchronization not exercised.' };
  });
  await test('stale warning formats are observed, repeat exact URI/position is deduplicated', async () => {
    const env = environment(); await env.activate(); const doc = makeDoc();
    for (const contents of ['AETHER WARNING: SIR is stale', { value: 'AETHER WARNING: SIR is stale' }, [{ language: 'rust', value: 'AETHER WARNING: SIR is stale' }]]) await env.hover(doc, position, token(), async () => ({ contents }));
    assert.match(env.status.text, /stale:1/); await env.hover(doc, { line: 0, character: 19 }, token(), async () => stale); assert.match(env.status.text, /stale:2/); await env.dispose();
    return { countingUnit: 'URI + requested cursor position, not distinct stale symbol or file' };
  });
  await test('metadata watcher is scoped to root A and records index activity', async () => {
    const env = environment({ workspaceFolders: [workspaceA, workspaceB] }); await env.activate(); assert.equal(env.state.watchers.length, 2); const watcher = env.state.watchers[0]; assert.strictEqual(watcher.pattern.base, workspaceA); assert.equal(watcher.pattern.pattern, '.aether/meta.sqlite'); watcher.change.emit(uri(path.join(workspaceAPath, '.aether', 'meta.sqlite'))); assert.match(env.status.text, /indexing/); await env.advance(5001); assert.match(env.status.text, /idle/); await env.dispose();
  });
  await test('fresh hover preserves documented session observation count', async () => {
    const env = environment(); await env.activate(); const doc = makeDoc(); await env.hover(doc, position, token(), async () => stale); await env.hover(doc, position, token(), async () => fixtureHover(doc)); const actual = env.status.text; await env.dispose(); assert.match(actual, /stale:1/, 'expected historical observation after fresh explanation: ' + actual);
  }, 'README documents warnings observed in this VS Code session, not current stale inventory.');
  await test('metadata update preserves documented session observation count', async () => {
    const env = environment(); await env.activate(); const doc = makeDoc(); await env.hover(doc, position, token(), async () => stale); env.state.watchers[0].change.emit(uri(path.join(workspaceAPath, '.aether', 'meta.sqlite'))); await env.hover(doc, position, token(), async () => fixtureHover(doc)); await env.advance(5001); const actual = env.status.text; await env.dispose(); assert.match(actual, /stale:1/, 'expected historical observation after metadata update: ' + actual);
  }, 'README documents historical observations; current hover freshness is checked separately.');
  await test('workspace callback preserves documented session observation count', async () => {
    const env = environment(); await env.activate(); await env.hover(makeDoc(), position, token(), async () => stale); await env.folders([workspaceB]); const actual = env.status.text; const tooltip = env.status.tooltip; assert.equal(env.state.clients.length, 2); assert.equal(env.state.clients[0].stops, 1); assert.match(tooltip, /Workspace: B/); await env.dispose(); assert.match(actual, /stale:1/, 'expected session-wide observation after workspace callback: ' + actual);
  }, 'Count is documented session-wide; this callback test is not a real host workspace restart.');
  await test('multi-root hover isolates root B from root A backend workspace', async () => {
    const env = environment({ workspaceFolders: [workspaceA, workspaceB] }); await env.activate(); const docB = makeDoc(workspaceB, sourceB); await env.hover(docB, position, token(), async () => fixtureHover(docB)); const client = env.state.clients[0]; const selectedRoot = client.serverOptions.args[client.serverOptions.args.indexOf('--workspace') + 1]; const workspaceScoped = client.clientOptions.workspaceFolder?.uri?.fsPath === workspaceAPath || client.clientOptions.documentSelector.every((selector) => !!selector.pattern); const evidence = { selectedRoot, hoveredRoot: workspaceBPath, workspaceFolderOption: client.clientOptions.workspaceFolder ?? null, documentSelector: client.clientOptions.documentSelector }; await env.dispose(); assert.ok(selectedRoot === workspaceBPath || workspaceScoped, 'root B hover routed through sole root A client with unrestricted selectors: ' + JSON.stringify(evidence));
  }, 'Separate workspaces with same relative paths/symbol names must not share a root-specific backend.');
  await test('missing workspace activation reports precise error and does not start client/process', async () => {
    const env = environment({ workspaceFolders: [] }); await env.activate(); assert.equal(env.state.clients.length, 0); assert.equal(env.state.spawns.length, 0); assert.deepEqual(env.state.errors, ['AETHER: Open a workspace folder first.']); assert.match(env.status.text, /no workspace/); await env.dispose();
  });
  await test('missing binary and unavailable cargo reports controlled preparation failure', async () => {
    const env = environment({ binaryExists: false }); await env.activate(); assert.equal(env.state.clients.length, 0); assert.equal(env.state.spawns.length, 1); assert.equal(env.state.spawns[0].command, 'cargo'); assert.deepEqual(Array.from(env.state.spawns[0].args), ['build', '-p', 'aetherd']); assert.match(env.state.errors[0], /Failed to prepare aetherd.*OFFLINE_FIXTURE: cargo unavailable/); assert.match(env.status.text, /error/); await env.dispose(); return { actualChildProcesses: 0, spawnIntercepted: 'cargo build -p aetherd' };
  });
  await test('language client start failure reports error and can retry via registered restart command', async () => {
    const error = new Error('OFFLINE_FIXTURE: aetherd launch rejected'); const env = environment({ startError: error }); await env.activate(); assert.equal(env.state.clients.length, 1); assert.match(env.state.errors[0], /Failed to start language client.*OFFLINE_FIXTURE/); assert.match(env.status.text, /error/); await env.state.commands.get('aether.restartServer')(); assert.equal(env.state.clients.length, 2); assert.equal(env.state.spawns.length, 0); await env.dispose();
  });
  await test('default activation explicitly requests indexing with auto provider and inherits environment', async () => {
    const fakeEnvironment = { GEMINI_API_KEY: 'OFFLINE_FIXTURE_NOT_A_REAL_KEY' }; const env = environment({ settings: { inferenceProvider: 'auto', inferenceModel: 'gemini-2.0-flash' }, environment: fakeEnvironment }); await env.activate(); const executable = env.state.clients[0].serverOptions;
    assert.ok(executable.args.includes('--index')); assert.equal(executable.args[executable.args.indexOf('--inference-provider') + 1], 'auto'); assert.equal(executable.args[executable.args.indexOf('--inference-model') + 1], 'gemini-2.0-flash'); assert.strictEqual(executable.options.env, fakeEnvironment); assert.equal(env.state.spawns.length, 0); await env.dispose();
    return { observation: 'Default activation asks the backend to index using auto provider and full inherited process environment. No real backend was started, so this does not assert the absence of activation-triggered model calls.' };
  });
  await test('deactivation disposes metadata watcher and status timer and stops exact client', async () => {
    const env = environment(); await env.activate(); await env.dispose(); assert.equal(env.state.clients[0].stops, 1); assert.equal(env.state.watchers[0].disposed, true); assert.equal(env.state.intervals[0].cleared, true);
  });
  await extraMultiRootTests();
  const report = {
    generatedAt: new Date().toISOString(),
    identity: { name: manifest.name, publisher: manifest.publisher, version: manifest.version, sourcePath, sourceSha256: sha256(source), transpiledSha256: sha256(compiled.outputText), typescriptVersion: ts.version },
    scope: 'src/extension.ts is transpiled unchanged and run in a VM against mocks of the vscode API, LanguageClient, fs.existsSync, child_process.spawn and the prompt-enhancer command. No process, network or extension host is used.',
    limitations: [
      'Hover contents come from fixtures, so this checks how the extension routes and forwards hovers, not what the backend explains.',
      'LanguageClient start and stop are mocked; real LSP wiring, URI conversion, unsaved edits, renames and rendering need an extension-host test.',
      'The Markdown check confirms an untrusted fixture stays untrusted; the middleware does not sanitize results the backend already marked trusted.',
    ],
    summary: { pass: results.filter((result) => result.status === 'PASS').length, fail: results.filter((result) => result.status === 'FAIL').length, total: results.length, realChildProcesses: 0 },
    results,
  };
  fs.writeFileSync(path.join(outDir, 'results.json'), JSON.stringify(report, null, 2) + '\n');
  const lines = [JSON.stringify(report.identity), report.scope, '', ...results.map((result) => result.status + ' ' + result.name + (result.error ? '\n  ' + result.error : '')), '', JSON.stringify(report.summary)];
  fs.writeFileSync(path.join(outDir, 'results.txt'), lines.join('\n') + '\n');
  console.log(lines.join('\n'));
  // Keep the report files only when the caller asked for an output directory.
  if (!process.argv[2]) fs.rmSync(outDir, { recursive: true, force: true });
  // Exit explicitly: a failing case can leave the extension's timers running.
  process.exit(report.summary.fail ? 1 : 0);
}
main().catch((error) => { console.error(error.stack); process.exit(2); });

function ownedClient(env,folder){return env.state.clients.find(c=>c.clientOptions.workspaceFolder.uri.toString()===folder.uri.toString()&&c.stops===0);}
function provider(client,doc,next){return client.clientOptions.middleware.provideHover(doc,position,token(),next);}
async function extraMultiRootTests(){
 await test('two roots use distinct clients and folder-bound options',async()=>{const env=environment({workspaceFolders:[workspaceA,workspaceB]});await env.activate();assert.equal(env.state.clients.length,2);for(const f of [workspaceA,workspaceB]){const c=ownedClient(env,f);assert(c);assert.equal(c.serverOptions.args[c.serverOptions.args.indexOf('--workspace')+1],f.uri.fsPath);assert(c.clientOptions.documentSelector.every(s=>typeof s.pattern==='string'&&s.pattern.endsWith('/**/*')));}assert.notEqual(env.state.clients[0].id,env.state.clients[1].id);await env.dispose();});
 await test('root A middleware refuses root B before calling backend',async()=>{const env=environment({workspaceFolders:[workspaceA,workspaceB]});await env.activate();let calls=0;const doc=makeDoc(workspaceB,sourceB);assert.equal(await provider(ownedClient(env,workspaceA),doc,async()=>{calls++;return fixtureHover(doc);}),undefined);assert.equal(calls,0);assert.match((await provider(ownedClient(env,workspaceB),doc,async()=>fixtureHover(doc))).contents[0].value,/difference/);await env.dispose();});
 await test('a sibling prefix is outside the folder boundary',async()=>{const env=environment();await env.activate();let calls=0;const doc=document(workspaceAPath+'-other/math.ts',sourceB);assert.equal(await provider(ownedClient(env,workspaceA),doc,async()=>{calls++;return fixtureHover(doc);}),undefined);assert.equal(calls,0);await env.dispose();});
 await test('adding root B keeps the running root A client',async()=>{const env=environment();await env.activate();const a=ownedClient(env,workspaceA);await env.folders([workspaceA,workspaceB]);assert.equal(env.state.clients.length,2);assert.strictEqual(ownedClient(env,workspaceA),a);assert.equal(a.starts,1);assert.equal(a.stops,0);assert.equal(env.state.watchers.length,2);await env.dispose();});
 await test('removing root B stops only B and disposes its watcher',async()=>{const env=environment({workspaceFolders:[workspaceA,workspaceB]});await env.activate();const a=ownedClient(env,workspaceA),b=ownedClient(env,workspaceB);await env.folders([workspaceA]);assert.equal(a.stops,0);assert.equal(b.stops,1);assert(env.state.watchers[1].disposed);assert(!env.state.watchers[0].disposed);await env.dispose();});
 await test('nested folders use only their most specific owner',async()=>{const nested=folder(path.join(workspaceAPath,'nested'),'nested',1),env=environment({workspaceFolders:[workspaceA,nested]});await env.activate();const doc=makeDoc(nested,sourceB);let outerCalls=0;assert.equal(await provider(ownedClient(env,workspaceA),doc,async()=>{outerCalls++;return fixtureHover(doc);}),undefined);assert.equal(outerCalls,0);assert.match((await provider(ownedClient(env,nested),doc,async()=>fixtureHover(doc))).contents[0].value,/difference/);await env.dispose();});
 await test('an in-flight hover is discarded when its root is removed',async()=>{const env=environment({workspaceFolders:[workspaceA,workspaceB]});await env.activate();let resolve;const doc=makeDoc(workspaceB,sourceB),pending=provider(ownedClient(env,workspaceB),doc,()=>new Promise(r=>resolve=r));await env.folders([workspaceA]);resolve(stale);assert.equal(await pending,undefined);assert.doesNotMatch(env.status.text,/stale:/);await env.dispose();});
 await test('rapid remove/re-add retires the old client generation',async()=>{const env=environment({workspaceFolders:[workspaceA,workspaceB]});await env.activate();const old=ownedClient(env,workspaceB);let resolve;const doc=makeDoc(workspaceB,sourceB),pending=provider(old,doc,()=>new Promise(r=>resolve=r));const removed=env.folders([workspaceA]),added=env.folders([workspaceA,workspaceB]);await Promise.all([removed,added]);resolve(stale);assert.equal(await pending,undefined);assert.equal(old.stops,1);assert.notStrictEqual(ownedClient(env,workspaceB),old);assert.equal(env.state.clients.length,3);await env.dispose();});
 await test('a root removed during startup is stopped before publication',async()=>{let release;const gate=new Promise(r=>release=r);const env=environment({workspaceFolders:[workspaceA,workspaceB],onStart:async c=>{if(c.clientOptions.workspaceFolder.uri.toString()===workspaceB.uri.toString())await gate;}});const activating=env.activate();for(let i=0;i<1000&&env.state.clients.length<2;i++)await Promise.resolve();assert.equal(env.state.clients.length,2,'both roots should be starting a client');const change=env.folders([workspaceA]);release();await Promise.all([activating,change]);assert.equal(env.state.clients[1].stops,1);assert.equal(env.state.clients[0].stops,0);await env.dispose();});
 await test('failed stop retains its handle and is retried without duplicate client',async()=>{let fail=true;const env=environment({workspaceFolders:[workspaceA,workspaceB],onStop:async c=>{if(c.clientOptions.workspaceFolder.uri.toString()===workspaceB.uri.toString()&&fail){fail=false;throw Error('fixture stop failure');}}});await env.activate();const b=ownedClient(env,workspaceB);await env.folders([workspaceA]);assert.equal(b.stops,1);assert(env.state.outputs.some(s=>s.includes('Failed to stop LSP for B')));await env.folders([workspaceA]);assert.equal(b.stops,2);assert.equal(env.state.clients.length,2);await env.dispose();});
  await test('restart recovers a folder whose client failed to start', async () => {
    let failures = 1;
    const env = environment({
      workspaceFolders: [workspaceA, workspaceB],
      onStart: async (c) => {
        if (c.clientOptions.workspaceFolder.uri.toString() === workspaceA.uri.toString() && failures > 0) {
          failures--;
          throw Error('fixture A startup failure');
        }
      },
    });
    await env.activate();
    assert.equal(ownedClient(env, workspaceA), undefined);
    await env.state.commands.get('aether.restartServer')();
    const a = ownedClient(env, workspaceA);
    assert(a, 'restart should start a fresh client for root A');
    assert.equal(a.startFailed, false);
    assert(ownedClient(env, workspaceB), 'root B should be running again after restart');
    await env.folders([workspaceA, workspaceB]);
    assert.strictEqual(ownedClient(env, workspaceA), a);
    await env.dispose();
  });
 await test('root B startup failure leaves root A functional',async()=>{const env=environment({workspaceFolders:[workspaceA,workspaceB],onStart:async c=>{if(c.clientOptions.workspaceFolder.uri.toString()===workspaceB.uri.toString())throw Error('fixture B startup failure');}});await env.activate();const a=ownedClient(env,workspaceA);assert(a);const doc=makeDoc();assert.match((await provider(a,doc,async()=>fixtureHover(doc))).contents[0].value,/sum/);assert.equal(a.stops,0);assert.equal(env.state.clients[1].stops,1);await env.dispose();});
 await test('deactivation stops every client and disposes all folder watchers',async()=>{const env=environment({workspaceFolders:[workspaceA,workspaceB]});await env.activate();await env.dispose();assert(env.state.clients.every(c=>c.stops===1));assert(env.state.watchers.every(w=>w.disposed));});
}
