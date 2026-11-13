use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use aether_store::SymbolRecord;

use crate::enhance_ranking::compare_records;

/// A deliberately narrow hint request, never an alternative exact resolver.
pub(super) struct SuggestionQuery {
    owners: Vec<String>,
    pub(super) leaves: Vec<String>,
}

impl SuggestionQuery {
    pub(super) fn parse(query: &str) -> Option<Self> {
        if query.len() > 256 {
            return None;
        }
        let normalized = query.trim().replace("::", ".");
        let mut parts = normalized.split('.').collect::<Vec<_>>();
        if !(2..=8).contains(&parts.len()) || !parts.iter().all(|part| identifier(part)) {
            return None;
        }
        let leaf = parts.pop()?;
        let bytes = leaf.as_bytes();
        let mut separated = String::new();
        for (index, byte) in bytes.iter().copied().enumerate() {
            let boundary = index > 0
                && byte.is_ascii_uppercase()
                && (bytes[index - 1].is_ascii_lowercase()
                    || bytes[index - 1].is_ascii_digit()
                    || (bytes[index - 1].is_ascii_uppercase()
                        && bytes.get(index + 1).is_some_and(u8::is_ascii_lowercase)));
            if boundary {
                separated.push('_');
            }
            separated.push(char::from(byte));
        }
        let leaves = BTreeSet::from([
            leaf.to_owned(),
            separated.clone(),
            separated.to_ascii_lowercase(),
        ])
        .into_iter()
        .collect();
        Some(Self {
            owners: parts.into_iter().map(str::to_owned).collect(),
            leaves,
        })
    }

    pub(super) fn select(&self, records: Vec<SymbolRecord>, limit: usize) -> Vec<SymbolRecord> {
        let mut records = records
            .into_iter()
            .filter(|record| self.matches(record))
            .collect::<Vec<_>>();
        records.sort_by(compare_records);
        let mut seen = HashSet::new();
        records
            .into_iter()
            .filter(|record| seen.insert(record.id.clone()))
            .take(limit)
            .collect()
    }

    fn matches(&self, record: &SymbolRecord) -> bool {
        let normalized = record.qualified_name.replace("::", ".");
        let Some((owner, leaf)) = normalized.rsplit_once('.') else {
            return false;
        };
        if !self.leaves.iter().any(|variant| variant == leaf) {
            return false;
        }
        let owners = owner.split('.').collect::<Vec<_>>();
        let query_owners = self.owners.iter().map(String::as_str).collect::<Vec<_>>();
        if owners.ends_with(&query_owners) {
            return true;
        }
        // An omitted class is plausible only after a complete, independently
        // verified file-module anchor. Never discard an unknown class qualifier.
        let path = record.file_path.replace('\\', "/");
        let path = Path::new(&path).with_extension("");
        let modules = path
            .iter()
            .filter_map(|part| part.to_str())
            .collect::<Vec<_>>();
        (1..=owners.len())
            .any(|end| modules.ends_with(&owners[..end]) && owners[..end].ends_with(&query_owners))
    }
}

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && value.bytes().any(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, name: &str, file: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.into(),
            qualified_name: name.into(),
            file_path: file.into(),
            language: "python".into(),
            kind: "method".into(),
            signature_fingerprint: "fixture".into(),
            last_seen_at: 0,
        }
    }

    #[test]
    fn camel_leaf_variants_preserve_acronyms_and_segment_case() {
        let request = SuggestionQuery::parse("Renderer.renderNode").expect("qualified");
        assert!(request.leaves.contains(&"render_Node".into()));
        assert!(request.leaves.contains(&"render_node".into()));
        let request = SuggestionQuery::parse("Reader.readHTTPHeader").expect("qualified");
        assert!(request.leaves.contains(&"read_HTTP_Header".into()));
        assert!(request.leaves.contains(&"read_http_header".into()));
    }

    #[test]
    fn owner_is_preserved_and_module_hint_keeps_ambiguity() {
        let records = vec![
            record(
                "b",
                "quasar.packet::FastEmitter::emit_packet",
                "src/quasar/packet.py",
            ),
            record(
                "a",
                "quasar.packet::PacketEmitter::emit_packet",
                "src/quasar/packet.py",
            ),
        ];
        let request = SuggestionQuery::parse("PacketEmitter.emitPacket").expect("qualified");
        let selected = request.select(records.clone(), 3);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "a");
        let request = SuggestionQuery::parse("packet.emitPacket").expect("qualified");
        assert_eq!(request.select(records, 3).len(), 2);
    }

    #[test]
    fn unknown_owner_module_substrings_and_wrong_file_do_not_match() {
        let records = vec![
            record(
                "a",
                "quasar.packet::PacketEmitter::emit_packet",
                "src/other.py",
            ),
            record(
                "b",
                "quasar.packet_helpers::Emitter::emit_packet",
                "src/packet_helpers.py",
            ),
            record("c", "quasar.packet::Emitter::_emit_packet", "src/packet.py"),
        ];
        for query in [
            "packet.emitPacket",
            "MissingEmitter.emitPacket",
            "Packetemitter.emitPacket",
        ] {
            assert!(
                SuggestionQuery::parse(query)
                    .expect("qualified")
                    .select(records.clone(), 3)
                    .is_empty()
            );
        }
    }

    #[test]
    fn filtering_precedes_limit_and_order_is_deterministic() {
        let mut records = (0..100)
            .map(|i| {
                record(
                    &format!("d{i}"),
                    &format!("other::A{i}::emit_packet"),
                    "other.py",
                )
            })
            .collect::<Vec<_>>();
        records.push(record(
            "target",
            "quasar.packet::Emitter::emit_packet",
            "src/quasar/packet.py",
        ));
        records.push(records.last().expect("target").clone());
        let request = SuggestionQuery::parse("packet.emitPacket").expect("qualified");
        let forward = request.select(records.clone(), 1);
        records.reverse();
        assert_eq!(forward, request.select(records, 1));
        assert_eq!(forward[0].id, "target");
    }

    #[test]
    fn bare_malformed_unicode_and_oversized_queries_are_not_expanded() {
        for query in [
            "emitPacket",
            ".emitPacket",
            "packet..emitPacket",
            "packet/r.emitPacket",
            "packet.émitPacket",
        ] {
            assert!(SuggestionQuery::parse(query).is_none());
        }
        assert!(SuggestionQuery::parse(&format!("packet.{}", "a".repeat(256))).is_none());
    }
}
