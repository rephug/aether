#!/usr/bin/env node
const fs = require('fs');
const path = require('path');

const root = path.resolve(__dirname, '..');
const packagePath = path.join(root, 'package.json');
const distPath = path.join(root, 'dist', 'extension.js');

const requiredCommands = [
  'aether.restartServer',
  'aether.selectInferenceProvider',
  'aether.indexOnce',
  'aether.searchSymbols',
  'aether.openSymbolResult',
];

const requiredActivationEvents = [
  'onCommand:aether.restartServer',
  'onCommand:aether.selectInferenceProvider',
  'onCommand:aether.indexOnce',
  'onCommand:aether.searchSymbols',
  'onCommand:aether.openSymbolResult',
];

function fail(message) {
  console.error(`smoke: ${message}`);
  process.exit(1);
}

if (!fs.existsSync(packagePath)) {
  fail('missing package.json');
}

if (!fs.existsSync(distPath)) {
  fail('missing dist/extension.js (run npm run build first)');
}

const pkg = JSON.parse(fs.readFileSync(packagePath, 'utf8'));

if (pkg.main !== './dist/extension.js') {
  fail(`unexpected main entry: ${String(pkg.main)}`);
}

const contributedCommands = new Set(
  (pkg.contributes?.commands ?? [])
    .map((entry) => entry?.command)
    .filter((command) => typeof command === 'string')
);

for (const commandId of requiredCommands) {
  if (!contributedCommands.has(commandId)) {
    fail(`missing contributes.commands entry: ${commandId}`);
  }
}

const activationEvents = new Set(
  (pkg.activationEvents ?? []).filter((event) => typeof event === 'string')
);

for (const eventName of requiredActivationEvents) {
  if (!activationEvents.has(eventName)) {
    fail(`missing activation event: ${eventName}`);
  }
}

console.log('smoke: ok');
