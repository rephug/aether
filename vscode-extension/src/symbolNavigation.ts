import * as vscode from 'vscode';

interface DefinitionCandidate {
  names: string[];
  range: vscode.Range;
}

function nameSegments(name: string): string[] {
  return name.split(/::|\.|#/)
    .map((part) => part.replace(/\(.*$/, '').replace(/<.*$/, '').trim())
    .filter((part) => part.length > 0);
}

/**
 * The name path an outline node contributes. rust-analyzer groups methods under
 * `impl Calc` or `impl Display for Calc` nodes, while AETHER names those methods
 * `Calc::alpha`, so an impl node stands for its self type.
 */
function containerSegments(name: string): string[] {
  const trimmed = name.trim();
  if (!/^impl\b/.test(trimmed)) {
    return nameSegments(trimmed);
  }
  let rest = trimmed.slice('impl'.length).trimStart();
  if (rest.startsWith('<')) {
    // Skip the impl's own generic parameters, which may nest.
    let depth = 0;
    let index = 0;
    for (; index < rest.length; index++) {
      if (rest[index] === '<') {
        depth++;
      } else if (rest[index] === '>' && --depth === 0) {
        break;
      }
    }
    rest = rest.slice(index + 1);
  }
  const forIndex = rest.lastIndexOf(' for ');
  const selfType = (forIndex >= 0 ? rest.slice(forIndex + ' for '.length) : rest)
    .trim()
    .replace(/^&\s*(mut\s+)?/, '');
  return nameSegments(selfType);
}

/**
 * How well a candidate's name path fits the requested one: `undefined` unless one
 * path ends with the other (AETHER's Python names carry module segments an outline
 * lacks; an outline may nest deeper than a qualified name says). Higher is better:
 * an exact path beats a partial one, then the longer shared suffix wins.
 */
function matchScore(requested: string[], names: string[]): number | undefined {
  const shared = Math.min(requested.length, names.length);
  for (let offset = 1; offset <= shared; offset++) {
    if (requested[requested.length - offset] !== names[names.length - offset]) {
      return undefined;
    }
  }
  const exact = requested.length === names.length;
  return (exact ? 1_000 : 0) + shared;
}

/**
 * Selects the declaration of `qualifiedName` in the editor's document, using the
 * document symbols a language extension provides. Search results carry names, not
 * positions, and the first text occurrence of a name is often a call site or a comment,
 * so there is no text-search fallback: returns `false` (and leaves the selection alone)
 * when no provider answers, the answer is ambiguous, or the document changed meanwhile.
 */
export async function revealSymbolDefinition(
  editor: vscode.TextEditor,
  qualifiedName: string
): Promise<boolean> {
  const document = editor.document;
  const version = document.version;
  const requested = nameSegments(qualifiedName);
  if (requested.length === 0) {
    return false;
  }

  let symbols: (vscode.DocumentSymbol | vscode.SymbolInformation)[] | undefined;
  try {
    symbols = await vscode.commands.executeCommand<
      (vscode.DocumentSymbol | vscode.SymbolInformation)[]
    >('vscode.executeDocumentSymbolProvider', document.uri);
  } catch {
    return false;
  }
  if (!symbols || document.isClosed || document.version !== version) {
    return false;
  }

  const candidates: DefinitionCandidate[] = [];
  const visit = (
    entries: (vscode.DocumentSymbol | vscode.SymbolInformation)[],
    parents: string[]
  ): void => {
    for (const entry of entries) {
      if ('selectionRange' in entry) {
        const names = [...parents, ...containerSegments(entry.name)];
        // An `impl` block only groups methods; it isn't a declaration of the type.
        if (!/^impl\b/.test(entry.name.trim())) {
          candidates.push({ names, range: entry.selectionRange });
        }
        visit(entry.children, names);
      } else if (entry.location.uri.toString() === document.uri.toString()) {
        candidates.push({
          names: [...containerSegments(entry.containerName), ...nameSegments(entry.name)],
          range: entry.location.range,
        });
      }
    }
  };
  visit(symbols, []);

  const scored = candidates
    .map((candidate) => ({ ...candidate, score: matchScore(requested, candidate.names) }))
    .filter((candidate) => candidate.score !== undefined);
  const best = Math.max(...scored.map((candidate) => candidate.score ?? 0));
  const matches = scored.filter((candidate) => candidate.score === best);
  // Several providers may report the same declaration; collapse identical ranges.
  // Distinct ranges (overloads, same-named items) stay ambiguous.
  const ranges = matches.map(({ range }) => range).filter((range, index, all) =>
    all.findIndex((other) => other.isEqual(range)) === index
  );
  if (ranges.length !== 1) {
    return false;
  }
  const range = ranges[0];
  if (range.isEmpty || !document.validateRange(range).isEqual(range)) {
    return false;
  }
  editor.selection = new vscode.Selection(range.start, range.end);
  editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
  return true;
}
