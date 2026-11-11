//! Distinguish Python declarations that otherwise share a signature identity.
//! Keep the historical fingerprint for every unambiguous declaration. For a
//! collision family only, include structural headers, branch roles and decorators;
//! never use body text, source offsets or occurrence ordinals as identity.

use std::collections::HashMap;

use aether_core::{Symbol, signature_fingerprint, stable_symbol_id};
use tree_sitter::Node;

pub(crate) fn disambiguate_symbol_identities(
    symbols: &mut [Symbol],
    root: Node<'_>,
    source: &[u8],
) {
    let mut families = HashMap::<String, Vec<usize>>::new();
    for (index, symbol) in symbols.iter().enumerate() {
        families.entry(symbol.id.clone()).or_default().push(index);
    }
    for family in families.values().filter(|family| family.len() > 1) {
        for &index in family {
            let symbol = &mut symbols[index];
            let (Some(start), Some(end)) = (symbol.range.start_byte, symbol.range.end_byte) else {
                continue;
            };
            let Some(node) = root.descendant_for_byte_range(start, end) else {
                continue;
            };
            if node.start_byte() != start || node.end_byte() != end {
                continue;
            }
            let context = structural_context(node, source);
            symbol.signature_fingerprint = signature_fingerprint(&format!(
                "python-declaration-context-v1\n{}\n{}",
                symbol.signature_fingerprint,
                blake3::hash(&context).to_hex(),
            ));
            symbol.id = stable_symbol_id(
                symbol.language,
                &symbol.file_path,
                symbol.kind,
                &symbol.qualified_name,
                &symbol.signature_fingerprint,
            );
        }
    }
    // The caller rejects any remaining duplicate IDs before edge extraction or
    // map projection. Repeated identical contexts must not silently pick a winner.
}

fn structural_context(mut node: Node<'_>, source: &[u8]) -> Vec<u8> {
    let mut context = Vec::new();
    let mut child_kind = "declaration";
    loop {
        let mut cursor = node.walk();
        let header_end = if node.kind() == "decorated_definition" {
            node.child_by_field_name("definition")
                .map(|definition| definition.start_byte())
        } else {
            node.named_children(&mut cursor)
                .find(|child| child.kind() == "block")
                .map(|body| body.start_byte())
        };
        if let Some(header_end) = header_end {
            append_part(&mut context, node.kind().as_bytes());
            append_part(&mut context, child_kind.as_bytes());
            append_header_tokens(node, header_end, source, &mut context);
        }
        let Some(parent) = node.parent() else {
            break;
        };
        child_kind = node.kind();
        node = parent;
    }
    context
}

// Tree-sitter terminal tokens preserve literal whitespace while excluding
// formatting between tokens and comments. Thus `if mode == "a b"` and
// `if mode == "ab"` remain distinct, and blank lines cannot change an ID.
fn append_header_tokens(node: Node<'_>, end: usize, source: &[u8], context: &mut Vec<u8>) {
    if node.start_byte() >= end || node.kind() == "comment" {
        return;
    }
    if node.child_count() == 0 {
        if node.end_byte() <= end
            && let Some(text) = source.get(node.byte_range())
        {
            append_part(context, node.kind().as_bytes());
            append_part(context, text);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        append_header_tokens(child, end, source, context);
    }
}

fn append_part(context: &mut Vec<u8>, part: &[u8]) {
    context.extend_from_slice(&(part.len() as u64).to_le_bytes());
    context.extend_from_slice(part);
}
