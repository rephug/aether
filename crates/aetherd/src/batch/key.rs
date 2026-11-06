//! Batch request keys, including legacy pipe-delimited fingerprints.
use anyhow::{Result, anyhow};

pub(crate) fn key_has_build_id(key: &str) -> bool {
    key_build_id(key).is_some()
}

pub(crate) fn key_build_id(key: &str) -> Option<&str> {
    split_request_key(key)
        .2
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

pub(crate) fn parse_key(key: &str) -> Result<(&str, &str)> {
    let (symbol, hash, _) = split_request_key(key);
    let (symbol, hash) = (symbol.trim(), hash.trim());
    if symbol.is_empty() || hash.is_empty() {
        return Err(anyhow!(
            "batch response key is missing symbol_id or prompt_hash"
        ));
    }
    Ok((symbol, hash))
}

/// New v3 hashes occupy one token. An old hash has three 16-hex tokens inside
/// the request key; retain all three before reading an optional build ID.
fn split_request_key(key: &str) -> (&str, &str, Option<&str>) {
    let (symbol, rest) = key.split_once('|').unwrap_or((key, "unknown"));
    let segments = rest.split('|').take(3).collect::<Vec<_>>();
    if segments.len() == 3
        && segments
            .iter()
            .all(|s| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        let hash_len = segments.iter().map(|s| s.len()).sum::<usize>() + 2;
        return (symbol, &rest[..hash_len], rest.get(hash_len + 1..));
    }
    let (hash, build) = rest
        .split_once('|')
        .map_or((rest, None), |(hash, build)| (hash, Some(build)));
    (symbol, hash, build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_versioned_and_legacy_segmented_fingerprints_round_trip() {
        for hash in [
            "1111111111111111|2222222222222222|3333333333333333",
            "v3:1111111111111111:2222222222222222:3333333333333333",
            "simple-legacy-hash",
        ] {
            let without_build = format!("symbol|{hash}");
            let with_build = format!("symbol|{hash}|build-1");
            assert_eq!(parse_key(&without_build).expect("key"), ("symbol", hash));
            assert!(!key_has_build_id(&without_build));
            assert_eq!(parse_key(&with_build).expect("key"), ("symbol", hash));
            assert_eq!(key_build_id(&with_build), Some("build-1"));
        }
    }

    #[test]
    fn preserves_missing_hash_fallback_and_rejects_empty_components() {
        assert_eq!(parse_key("symbol").expect("legacy"), ("symbol", "unknown"));
        for key in ["|hash", "symbol|", " |hash"] {
            assert!(parse_key(key).is_err());
        }
    }
}
