/* Regression tests for src/symbolNavigation.ts: the module is transpiled unchanged and
   run against a small mock of the vscode API, with document symbols supplied per case. */
'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const assert = require('assert/strict');
const ts = require('../node_modules/typescript');

const sourcePath = path.resolve(__dirname, '../src/symbolNavigation.ts');
const compiled = ts.transpileModule(fs.readFileSync(sourcePath, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
  fileName: sourcePath,
});

class Position {
  constructor(line, character) { this.line = line; this.character = character; }
  isEqual(other) { return this.line === other.line && this.character === other.character; }
}
class Range {
  constructor(start, end) { this.start = start; this.end = end; }
  get isEmpty() { return this.start.isEqual(this.end); }
  isEqual(other) { return this.start.isEqual(other.start) && this.end.isEqual(other.end); }
}
class Selection extends Range {}
const range = (line, from, to) => new Range(new Position(line, from), new Position(line, to));

// The provider answer for the next `executeDocumentSymbolProvider` call.
let provide = async () => undefined;
const vscode = {
  Position,
  Range,
  Selection,
  TextEditorRevealType: { InCenterIfOutsideViewport: 2 },
  commands: {
    executeCommand: async (command, uri) => {
      assert.equal(command, 'vscode.executeDocumentSymbolProvider');
      return provide(uri);
    },
  },
};

const sandboxModule = { exports: {} };
vm.runInNewContext(compiled.outputText, {
  module: sandboxModule,
  exports: sandboxModule.exports,
  require: (name) => {
    if (name === 'vscode') return vscode;
    throw new Error(`unexpected require: ${name}`);
  },
});
const { revealSymbolDefinition } = sandboxModule.exports;

const SOURCE = [
  'fn run() -> i32 {', //                          0
  '    helper()', //                                1  call site: the first text match
  '}', //                                           2
  '', //                                            3
  'fn helper() -> i32 { 1 }', //                    4  declaration
  'struct Calc;', //                                5
  'impl Calc { fn alpha(&self) {} }', //            6
  'impl Display for Calc { fn fmt(&self) {} }', //  7
  'fn alpha() {}', //                               8
  'mod a { fn dup() {} }', //                       9
  'mod b { fn dup() {} }', //                       10
].join('\n');

function editorFor(text = SOURCE, uri = 'file:///work/src/lib.rs') {
  const lines = text.split('\n');
  const document = {
    uri: { toString: () => uri },
    version: 1,
    isClosed: false,
    getText: () => text,
    // Clamps a range to the document, like vscode.TextDocument.validateRange.
    validateRange(r) {
      const clamp = (p) => {
        const line = Math.min(Math.max(p.line, 0), lines.length - 1);
        return new Position(line, Math.min(Math.max(p.character, 0), lines[line].length));
      };
      return new Range(clamp(r.start), clamp(r.end));
    },
  };
  const initial = new Selection(new Position(0, 0), new Position(0, 0));
  return { document, selection: initial, initial, revealed: [], revealRange(r) { this.revealed.push(r); } };
}

function documentSymbol(name, selectionRange, children = []) {
  return { name, selectionRange, range: selectionRange, children };
}

// The outline rust-analyzer reports for SOURCE: methods sit under `impl` nodes.
const runSymbol = documentSymbol('run', range(0, 3, 6));
const helperSymbol = documentSymbol('helper', range(4, 3, 9));
const calcStruct = documentSymbol('Calc', range(5, 7, 11));
const calcAlpha = documentSymbol('alpha', range(6, 15, 20));
const calcFmt = documentSymbol('fmt', range(7, 27, 30));
const freeAlpha = documentSymbol('alpha', range(8, 3, 8));
const allSymbols = [
  runSymbol,
  helperSymbol,
  calcStruct,
  documentSymbol('impl Calc', range(6, 0, 9), [calcAlpha]),
  documentSymbol('impl Display for Calc', range(7, 0, 21), [calcFmt]),
  freeAlpha,
  documentSymbol('a', range(9, 4, 5), [documentSymbol('dup', range(9, 11, 14))]),
  documentSymbol('b', range(10, 4, 5), [documentSymbol('dup', range(10, 11, 14))]),
];

const results = [];
async function test(name, body) {
  try { await body(); results.push(`PASS ${name}`); }
  catch (error) { results.push(`FAIL ${name}\n  ${error.message}`); }
}

function assertSelected(editor, expected) {
  assert(editor.selection.isEqual(expected), `selected ${JSON.stringify(editor.selection)}`);
  assert.equal(editor.revealed.length, 1);
}
function assertUntouched(editor) {
  assert.strictEqual(editor.selection, editor.initial);
  assert.equal(editor.revealed.length, 0);
}

async function main() {
  await test('opens the declaration, not the earlier call site', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    // The previous text search stopped at the first word match: the call on line 1.
    const firstTextMatch = SOURCE.split('\n').findIndex((line) => /\bhelper\b/.test(line));
    assert.equal(firstTextMatch, 1);
    assert.equal(await revealSymbolDefinition(editor, 'helper'), true);
    assertSelected(editor, helperSymbol.selectionRange);
  });

  await test('call-style and generic suffixes are ignored when matching', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'helper()'), true);
    assertSelected(editor, helperSymbol.selectionRange);
  });

  await test('a method in an impl block is found by its type-qualified name', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'Calc::alpha'), true);
    assertSelected(editor, calcAlpha.selectionRange);
  });

  await test('a trait method implemented for a type is found the same way', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'Calc::fmt'), true);
    assertSelected(editor, calcFmt.selectionRange);
  });

  await test('a type name selects the type, not its impl blocks', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'Calc'), true);
    assertSelected(editor, calcStruct.selectionRange);
  });

  await test('an exact top-level name wins over a same-named method', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'alpha'), true);
    assertSelected(editor, freeAlpha.selectionRange);
  });

  await test('a bare name declared in two modules is ambiguous', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'dup'), false);
    assertUntouched(editor);
  });

  await test('module-qualified Python names match an outline without the module', async () => {
    const pyAlpha = documentSymbol('alpha', range(2, 8, 13));
    const pyFree = documentSymbol('alpha', range(5, 4, 9));
    provide = async () => [documentSymbol('Calc', range(0, 6, 10), [pyAlpha]), pyFree];
    const pySource = [
      'class Calc:', //                 0
      '    def __init__(self): pass', // 1
      '    def alpha(self):', //        2
      '        return 1', //            3
      '', //                            4
      'def alpha():', //                5
      '    return 2', //                6
    ].join('\n');
    let editor = editorFor(pySource, 'file:///work/src/pkg/mod.py');
    assert.equal(await revealSymbolDefinition(editor, 'src.pkg.mod::Calc::alpha'), true);
    assertSelected(editor, pyAlpha.selectionRange);
    editor = editorFor(pySource, 'file:///work/src/pkg/mod.py');
    assert.equal(await revealSymbolDefinition(editor, 'src.pkg.mod::alpha'), true);
    assertSelected(editor, pyFree.selectionRange);
  });

  await test('flat symbol information matches through its container', async () => {
    const editor = editorFor();
    provide = async () => [
      { name: 'alpha', containerName: 'impl Calc', location: { uri: editor.document.uri, range: range(6, 15, 20) } },
      { name: 'alpha', containerName: 'Calc', location: { uri: { toString: () => 'file:///work/src/other.rs' }, range: range(2, 3, 8) } },
    ];
    assert.equal(await revealSymbolDefinition(editor, 'Calc::alpha'), true);
    assertSelected(editor, range(6, 15, 20));
  });

  await test('the same declaration reported twice is not ambiguous', async () => {
    provide = async () => [helperSymbol, documentSymbol('helper', range(4, 3, 9))];
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'helper'), true);
    assertSelected(editor, helperSymbol.selectionRange);
  });

  await test('no provider answer leaves the selection alone', async () => {
    provide = async () => undefined;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'helper'), false);
    assertUntouched(editor);
  });

  await test('a failing provider leaves the selection alone', async () => {
    provide = async () => { throw new Error('provider failed'); };
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'helper'), false);
    assertUntouched(editor);
  });

  await test('an unknown name is not matched against text', async () => {
    provide = async () => allSymbols;
    const editor = editorFor();
    assert.equal(await revealSymbolDefinition(editor, 'missing'), false);
    assertUntouched(editor);
  });

  await test('an edit during the request makes the answer stale', async () => {
    const editor = editorFor();
    provide = async () => { editor.document.version += 1; return allSymbols; };
    assert.equal(await revealSymbolDefinition(editor, 'helper'), false);
    assertUntouched(editor);
  });

  await test('a document closed during the request is not touched', async () => {
    const editor = editorFor();
    provide = async () => { editor.document.isClosed = true; return allSymbols; };
    assert.equal(await revealSymbolDefinition(editor, 'helper'), false);
    assertUntouched(editor);
  });

  await test('ranges outside the document or empty are rejected', async () => {
    for (const bad of [range(40, 0, 5), range(4, 3, 3)]) {
      provide = async () => [documentSymbol('helper', bad)];
      const editor = editorFor();
      assert.equal(await revealSymbolDefinition(editor, 'helper'), false);
      assertUntouched(editor);
    }
  });

  const failed = results.filter((line) => line.startsWith('FAIL')).length;
  console.log(results.join('\n'));
  console.log(JSON.stringify({ pass: results.length - failed, fail: failed, total: results.length }));
  process.exit(failed ? 1 : 0);
}

main().catch((error) => { console.error(error.stack); process.exit(2); });
