/* Runs inside a real, isolated VS Code Extension Development Host.
   The launcher supplies a synthetic workspace and a mock-only daemon. */
'use strict';
const assert = require('assert/strict');
const fs = require('fs');
const path = require('path');
const vscode = require('vscode');

const textOf = hovers => hovers.flatMap(hover => hover.contents.map(content =>
  typeof content === 'string' ? content : content.value)).join('\n');

exports.run = async () => {
  const result = { kind: 'real VS Code extension host', plannedBackend: 'real stdio LSP with mock inference only', checks: [] };
  try {
    if (!vscode.workspace.isTrusted) {
      result.status = 'blocked';
      throw new Error('Test workspace is untrusted; no trust setting will be changed');
    }
    const extension = vscode.extensions.getExtension('aether.aether-vscode');
    assert(extension, 'AETHER development extension is available');
    await extension.activate();
    const root = vscode.workspace.workspaceFolders[0].uri.fsPath;
    const uri = vscode.Uri.file(path.join(root, 'demo.py'));
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc);
    assert.equal(doc.languageId, 'python');
    const hoverText = async () => textOf(await vscode.commands.executeCommand(
      'vscode.executeHoverProvider', uri, new vscode.Position(0, 6)) || []);
    let saved = '';
    const deadline = Date.now() + 60000;
    while (Date.now() < deadline) {
      saved = await hoverText();
      if (saved.includes('**What this does**') && saved.includes('**Technical details**')) break;
      await new Promise(resolve => setTimeout(resolve, 250));
    }
    assert.match(saved, /\*\*What this does\*\*/);
    assert.match(saved, /\*\*Technical details\*\*/);
    assert.match(saved, /\[MOCK\]/, 'A mock placeholder must remain visibly a placeholder');
    result.checks.push('saved Python hover returned through the real LSP');
    result.savedHover = saved;

    const edit = new vscode.WorkspaceEdit();
    edit.insert(uri, new vscode.Position(1, 0), '    # unsaved edit\n');
    assert(await vscode.workspace.applyEdit(edit));
    assert(doc.isDirty);
    const unsaved = await hoverText();
    assert.match(unsaved, /Save this file/);
    assert.doesNotMatch(unsaved, /\[MOCK\]/);
    result.unsavedHover = unsaved;
    result.checks.push('unsaved text shows the save message instead of the disk explanation');
    await doc.save();
    const afterSave = await hoverText();
    assert.doesNotMatch(afterSave, /Save this file/);
    assert.match(afterSave, /\*\*What this does\*\*/);
    assert.match(afterSave, /\*\*Technical details\*\*/);
    result.afterSaveHover = afterSave;
    result.checks.push('saving resumes the backend hover');
    result.status = 'passed';
  } catch (error) {
    result.status = result.status || 'failed'; result.error = error.stack;
    throw error;
  } finally {
    result.limits = 'Only the listed checks completed. Mock inference cannot assess LLM explanation accuracy or visual rendering.';
    fs.writeFileSync(process.env.AETHER_LIVE_HOVER_OUTPUT, JSON.stringify(result, null, 2));
    const extension = vscode.extensions.getExtension('aether.aether-vscode');
    if (extension?.isActive) {
      const api = require('../dist/extension.js');
      await api.deactivate();
    }
  }
};
