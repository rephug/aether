/* Optional live check: node test/run-live-hover.cjs <aetherd> <output-dir> [Code.exe] [--resume]
   Uses an installed VS Code, a separate profile, and synthetic SQLite data only.
   No workspace-trust or other security setting is changed. */
'use strict';
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');
const assert = require('assert/strict');

const binary = path.resolve(process.argv[2] || '');
const output = path.resolve(process.argv[3] || '');
const resume = process.argv.includes('--resume');
const code = process.argv.slice(4).find(arg => arg !== '--resume') || path.join(process.env.LOCALAPPDATA || '', 'Programs', 'Microsoft VS Code', 'Code.exe');
assert(process.argv[2] && process.argv[3], 'Supply an existing aetherd binary and a new output directory');
assert(fs.existsSync(binary) && fs.existsSync(code), 'aetherd and VS Code must already be installed');
assert(resume ? fs.existsSync(output) : !fs.existsSync(output), 'Use a new output directory, or --resume to reuse a previously prepared test profile');
const workspace = path.join(output, 'workspace');
const userData = path.join(output, 'profile');
const configPath = path.join(workspace, '.aether', 'config.toml');
const settingsPath = path.join(userData, 'User', 'settings.json');
const config = '[inference]\nprovider = "mock"\n[storage]\ngraph_backend = "sqlite"\n[embeddings]\nenabled = false\nvector_backend = "sqlite"\n';
if (!resume) {
  fs.mkdirSync(path.join(workspace, '.aether'), { recursive: true });
  fs.mkdirSync(path.join(userData, 'User'), { recursive: true });
  fs.writeFileSync(path.join(workspace, 'demo.py'), 'def increment(x):\n    return x + 1\n');
  fs.writeFileSync(configPath, config);
  fs.writeFileSync(settingsPath, JSON.stringify({ 'aether.inferenceProvider': 'mock', 'aether.aetherdPath': binary }));
} else {
  const settings = JSON.parse(fs.readFileSync(settingsPath, 'utf8'));
  assert.equal(settings['aether.inferenceProvider'], 'mock');
  assert.equal(settings['aether.aetherdPath'], binary);
  assert.equal(fs.readFileSync(configPath, 'utf8'), config, 'Only the mock-only test fixture can be resumed');
}
const suffix = resume ? '-' + Date.now() : '';
const resultFile = path.join(output, 'results' + suffix + '.json');
const log = fs.openSync(path.join(output, 'vscode' + suffix + '.log'), 'w');
const env = { ...process.env, AETHER_LIVE_HOVER_OUTPUT: resultFile };
delete env.ELECTRON_RUN_AS_NODE;
const child = spawn(code, [
  '--new-window', '--wait', '--skip-welcome', '--skip-release-notes', '--disable-extensions',
  '--user-data-dir', userData, '--extensions-dir', path.join(output, 'extensions'),
  '--extensionDevelopmentPath', path.resolve(__dirname, '..'),
  '--extensionTestsPath', path.join(__dirname, 'live-hover.cjs'), workspace,
], { env, windowsHide: true, stdio: ['ignore', log, log] });
const timer = setTimeout(() => { child.kill(); console.error('VS Code test timed out; inspect the isolated profile log.'); }, 90000);
child.once('error', error => { clearTimeout(timer); fs.closeSync(log); console.error(error.message); process.exitCode = 1; });
child.once('exit', code => {
  clearTimeout(timer); fs.closeSync(log);
  const result = fs.existsSync(resultFile) ? JSON.parse(fs.readFileSync(resultFile, 'utf8')) : { status: 'unverified', reason: 'No extension-host result returned' };
  console.log(JSON.stringify({ exitCode: code, output, resultFile, ...result }, null, 2));
  process.exitCode = result.status === 'passed' && code === 0 ? 0 : 1;
});
