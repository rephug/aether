/* Regression tests for how src/extension.ts finds the aetherd binary: the module is
   transpiled unchanged and its ensureAetherdBinary runs against mocks of fs.existsSync,
   child_process.spawn, process and the AETHER settings, so no cargo or aetherd runs. */
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const assert = require('assert/strict');
const { EventEmitter } = require('events');
const ts = require('../node_modules/typescript');

const sourcePath = path.resolve(__dirname, '../src/extension.ts');
const manifest = JSON.parse(fs.readFileSync(path.resolve(__dirname, '../package.json'), 'utf8'));
const compiled = ts.transpileModule(fs.readFileSync(sourcePath, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
  fileName: sourcePath,
});

const repoRoot = path.resolve('/fixture/repo');
const repoBinary = path.join(repoRoot, 'target', 'debug', 'aetherd');
const targetDir = path.resolve('/fixture/cargo-target');
const targetBinary = path.join(targetDir, 'debug', 'aetherd');

// Where `cargo build -p aetherd` writes the binary, given the spawn options it ran with.
function cargoOutput(options) {
  const dir = options.env.CARGO_TARGET_DIR
    ? path.resolve(options.cwd, options.env.CARGO_TARGET_DIR)
    : path.join(options.cwd, 'target');
  return path.join(dir, 'debug', 'aetherd');
}

/**
 * Loads the extension with `files` present on disk. `build` decides what a cargo build
 * creates; without it, spawning cargo fails as if cargo were not installed.
 */
function load({ env = {}, files = [], settings = {}, platform = 'linux', build } = {}) {
  const existing = new Set(files);
  const state = { spawns: [] };
  const fakeSpawn = (command, args, options) => {
    state.spawns.push({ command, args: Array.from(args), options });
    const child = new EventEmitter();
    child.stdout = new EventEmitter();
    child.stderr = new EventEmitter();
    queueMicrotask(() => {
      if (!build) {
        child.emit('error', new Error('OFFLINE_FIXTURE: cargo unavailable'));
        return;
      }
      for (const file of build(options)) existing.add(file);
      child.emit('close', 0);
    });
    return child;
  };
  const vscode = {
    workspace: {
      getConfiguration(section) {
        assert.equal(section, 'aether');
        return { get: (key, fallback) => (Object.hasOwn(settings, key) ? settings[key] : fallback) };
      },
    },
  };
  const exports = {};
  vm.runInNewContext(
    compiled.outputText + '\nexports.__ensureAetherdBinary = ensureAetherdBinary;\n',
    {
      exports,
      module: { exports },
      process: { platform, env },
      require(name) {
        if (name === 'vscode') return vscode;
        if (name === 'path') return path;
        if (name === 'fs') return { existsSync: (file) => existing.has(file) };
        if (name === 'child_process') return { spawn: fakeSpawn };
        if (name === 'vscode-languageclient/node') return { LanguageClient: class {} };
        if (name === './enhancePrompt') return { createEnhancePromptCommand: () => () => {} };
        if (name === './symbolNavigation') return { revealSymbolDefinition: async () => false };
        throw new Error(`unexpected require: ${name}`);
      },
    },
    { filename: `${sourcePath}.transpiled` }
  );
  return { state, ensure: () => exports.__ensureAetherdBinary(repoRoot) };
}

const results = [];
async function test(name, callback) {
  try {
    await callback();
    results.push(`PASS ${name}`);
  } catch (error) {
    results.push(`FAIL ${name}\n  ${error.message}`);
  }
}

async function main() {
  await test('a binary in CARGO_TARGET_DIR is used without running cargo', async () => {
    const env = load({ env: { CARGO_TARGET_DIR: targetDir }, files: [targetBinary] });
    assert.equal(await env.ensure(), targetBinary);
    assert.equal(env.state.spawns.length, 0);
  });

  await test('after cargo builds into CARGO_TARGET_DIR the built binary is returned', async () => {
    const env = load({ env: { CARGO_TARGET_DIR: targetDir }, build: (options) => [cargoOutput(options)] });
    assert.equal(await env.ensure(), targetBinary);
    assert.equal(env.state.spawns.length, 1);
  });

  await test('a relative CARGO_TARGET_DIR resolves against the repo root, as cargo does', async () => {
    const env = load({ env: { CARGO_TARGET_DIR: 'out' }, build: (options) => [cargoOutput(options)] });
    assert.equal(await env.ensure(), path.join(repoRoot, 'out', 'debug', 'aetherd'));
  });

  await test('CARGO_TARGET_DIR wins over an older <repo>/target binary', async () => {
    const env = load({ env: { CARGO_TARGET_DIR: targetDir }, files: [repoBinary, targetBinary] });
    assert.equal(await env.ensure(), targetBinary);
  });

  await test('<repo>/target is still used when CARGO_TARGET_DIR is unset', async () => {
    const env = load({ files: [repoBinary] });
    assert.equal(await env.ensure(), repoBinary);
    assert.equal(env.state.spawns.length, 0);
  });

  await test('without CARGO_TARGET_DIR the build output under <repo>/target is returned', async () => {
    const env = load({ build: (options) => [cargoOutput(options)] });
    assert.equal(await env.ensure(), repoBinary);
  });

  await test('cargo builds aetherd in the repo root with the inherited environment', async () => {
    const inherited = { CARGO_TARGET_DIR: targetDir };
    const env = load({ env: inherited, build: (options) => [cargoOutput(options)] });
    await env.ensure();
    const [spawned] = env.state.spawns;
    assert.equal(spawned.command, 'cargo');
    assert.deepEqual(spawned.args, ['build', '-p', 'aetherd']);
    assert.equal(spawned.options.cwd, repoRoot);
    assert.strictEqual(spawned.options.env, inherited);
  });

  await test('an aetherd on PATH is used before building', async () => {
    const env = load({ env: { PATH: '/fixture/bin:/fixture/tools' }, files: ['/fixture/tools/aetherd'] });
    assert.equal(await env.ensure(), '/fixture/tools/aetherd');
    assert.equal(env.state.spawns.length, 0);
  });

  await test('<repo>/target wins over PATH', async () => {
    const env = load({ env: { PATH: '/fixture/tools' }, files: [repoBinary, '/fixture/tools/aetherd'] });
    assert.equal(await env.ensure(), repoBinary);
  });

  await test('empty and relative PATH entries are not searched', async () => {
    const env = load({ env: { PATH: ':bin:./tools' }, files: ['aetherd', 'bin/aetherd', 'tools/aetherd', './tools/aetherd'] });
    await assert.rejects(env.ensure(), /OFFLINE_FIXTURE: cargo unavailable/);
    assert.equal(env.state.spawns.length, 1);
  });

  await test('Windows splits PATH on ; and looks for aetherd.exe', async () => {
    const env = load({ platform: 'win32', env: { PATH: '/fixture/bin;/fixture/tools' }, files: ['/fixture/tools/aetherd', '/fixture/tools/aetherd.exe'] });
    assert.equal(await env.ensure(), '/fixture/tools/aetherd.exe');
  });

  await test('the aether.aetherdPath setting wins over every other location', async () => {
    const env = load({
      env: { CARGO_TARGET_DIR: targetDir, PATH: '/fixture/tools' },
      settings: { aetherdPath: ' /fixture/custom/aetherd ' },
      files: ['/fixture/custom/aetherd', targetBinary, repoBinary, '/fixture/tools/aetherd'],
    });
    assert.equal(await env.ensure(), '/fixture/custom/aetherd');
    assert.equal(env.state.spawns.length, 0);
  });

  await test('a missing aether.aetherdPath is reported instead of falling back or building', async () => {
    const env = load({ settings: { aetherdPath: '/fixture/missing/aetherd' }, files: [repoBinary] });
    await assert.rejects(env.ensure(), /aether\.aetherdPath.*\/fixture\/missing\/aetherd/);
    assert.equal(env.state.spawns.length, 0);
  });

  await test('a relative aether.aetherdPath is rejected', async () => {
    const env = load({ settings: { aetherdPath: 'bin/aetherd' }, files: ['bin/aetherd', repoBinary] });
    await assert.rejects(env.ensure(), /aether\.aetherdPath.*absolute/);
    assert.equal(env.state.spawns.length, 0);
  });

  await test('a build that leaves no binary names the locations cargo could have used', async () => {
    const env = load({ env: { CARGO_TARGET_DIR: targetDir }, build: () => [] });
    await assert.rejects(env.ensure(), (error) => {
      assert.match(error.message, /not found after/);
      assert.ok(error.message.includes(targetBinary), error.message);
      assert.ok(error.message.includes(repoBinary), error.message);
      assert.match(error.message, /aether\.aetherdPath/);
      return true;
    });
  });

  await test('aether.aetherdPath is contributed as a machine-scoped string setting', async () => {
    const setting = manifest.contributes.configuration.properties['aether.aetherdPath'];
    assert.ok(setting, 'package.json does not contribute aether.aetherdPath');
    assert.equal(setting.type, 'string');
    assert.equal(setting.default, '');
    // A workspace must not be able to point the extension at a binary of its choosing.
    assert.equal(setting.scope, 'machine');
  });

  const failed = results.filter((line) => line.startsWith('FAIL')).length;
  console.log(results.join('\n'));
  console.log(JSON.stringify({ pass: results.length - failed, fail: failed, total: results.length }));
  process.exit(failed ? 1 : 0);
}

main().catch((error) => { console.error(error.stack); process.exit(2); });
