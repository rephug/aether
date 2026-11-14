use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use aether_core::{EdgeKind, SymbolEdge};
use aether_store::{
    SirMetaRecord, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRelationStore,
};
use tempfile::tempdir;
use tracing::dispatcher::{self, Dispatch};
use tracing_subscriber::fmt::MakeWriter;

use super::{
    EnhanceDocumentFormat, EnhanceRequest, TaskType, enhance_prompt_core,
    extract_intent_via_keywords, prompt_has_explicit_targets, read_sir_annotation,
    resolve_symbol_query,
};

#[derive(Clone, Default)]
struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter(self.0.clone())
    }
}

impl Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture_logs<T>(run: impl FnOnce() -> T) -> (T, String) {
    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_writer(buffer.clone())
        .finish();
    let result = dispatcher::with_default(&Dispatch::new(subscriber), run);
    let logs =
        String::from_utf8(buffer.0.lock().expect("log buffer lock").clone()).expect("utf8 logs");
    (result, logs)
}

fn write_test_config(workspace: &Path) {
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "gemini"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"

[health]
enabled = true

[coupling]
enabled = true
"#,
    )
    .expect("write config");
}

#[test]
fn exact_targets_survive_more_than_eight_broad_queries_in_either_order() {
    let workspace = tempfile::tempdir().expect("independent ranking workspace");
    write_test_config(workspace.path());
    let store = SqliteStore::open(workspace.path()).expect("synthetic catalog");
    let mut queries = Vec::new();
    for index in 0..12 {
        let query = format!("nebula{index}");
        seed_symbol(
            &store,
            &format!("decoy-{index}"),
            &format!("src/decoy{index}.rs"),
            &format!("{query}::catalog"),
            "function",
            "Independent package decoy.",
        );
        queries.push(query);
    }
    seed_symbol(
        &store,
        "packet-target",
        "src/packet.rs",
        "quasar::PacketReader::decode",
        "method",
        "Decode a public packet fixture.",
    );
    queries.push("PacketReader.decode".into());
    let mut intent = super::EnhanceIntent {
        target_symbols: queries,
        ..Default::default()
    };
    let first = super::assemble_context(workspace.path(), &store, &intent).expect("first order");
    intent.target_symbols.reverse();
    let reversed =
        super::assemble_context(workspace.path(), &store, &intent).expect("reversed order");
    assert_eq!(first.resolved_symbols[0], "quasar::PacketReader::decode");
    assert_eq!(
        first.resolved_symbols.len(),
        1 + super::FOCUSED_DISCOVERY_ALLOWANCE
    );
    assert_eq!(first.resolved_symbols, reversed.resolved_symbols);
    assert_eq!(first.referenced_files, reversed.referenced_files);
    assert_eq!(first.referenced_files[0], "src/packet.rs");
}

#[test]
fn incidental_lowercase_leaf_matches_keep_the_full_discovery_budget() {
    let workspace = tempfile::tempdir().expect("independent discovery workspace");
    write_test_config(workspace.path());
    let store = SqliteStore::open(workspace.path()).expect("synthetic catalog");
    let mut queries = Vec::new();
    for index in 0..12 {
        let query = format!("items{index}");
        seed_symbol(
            &store,
            &format!("item-{index}"),
            &format!("src/items{index}.rs"),
            &format!("catalog::{query}"),
            "function",
            "Independent discovery candidate.",
        );
        queries.push(query);
    }
    let intent = super::EnhanceIntent {
        raw_prompt: "Investigate items in the catalog".into(),
        target_symbols: queries,
        ..Default::default()
    };
    let result =
        super::assemble_context(workspace.path(), &store, &intent).expect("discovery context");
    assert_eq!(result.resolved_symbols.len(), super::MAX_SYMBOL_CONTEXTS);

    let focused_intent = super::EnhanceIntent {
        raw_prompt: "Investigate `items9` in the catalog".into(),
        ..intent
    };
    let focused = super::assemble_context(workspace.path(), &store, &focused_intent)
        .expect("explicitly quoted focus");
    assert_eq!(focused.resolved_symbols.len(), 3);
    assert_eq!(focused.resolved_symbols[0], "catalog::items9");
    let broad_text =
        super::render_enhanced_document("Inspect catalog", &result.context, 8_192, false)
            .expect("render broad discovery");
    let focused_text =
        super::render_enhanced_document("Inspect catalog", &focused.context, 8_192, false)
            .expect("render focused discovery");
    assert!(focused_text.len() < broad_text.len());
    println!(
        "focused disclosure bytes: broad={} focused={}",
        broad_text.len(),
        focused_text.len()
    );
}

#[test]
fn normalized_exact_name_precedes_owner_suffixes_before_resolver_limit() {
    let workspace = tempfile::tempdir().expect("independent owner workspace");
    let store = SqliteStore::open(workspace.path()).expect("synthetic catalog");
    for prefix in ["A", "B", "C", "D"] {
        seed_symbol(
            &store,
            prefix,
            "src/a.rs",
            &format!("{prefix}::Pack::Reader::decode"),
            "method",
            "Owner suffix decoy.",
        );
    }
    seed_symbol(
        &store,
        "exact-owner",
        "src/z.rs",
        "Pack::Reader::decode",
        "method",
        "Exact normalized target.",
    );
    for query in ["Pack.Reader.decode", "Pack::Reader::decode"] {
        let records = resolve_symbol_query(&store, query, 3).expect("resolve independently");
        assert_eq!(records[0].id, "exact-owner");
        assert!(records.len() <= 3);
    }
    assert!(
        resolve_symbol_query(&store, "MissingOwner.decode", 3)
            .expect("missing owner")
            .is_empty()
    );
}

#[test]
fn enhancement_renders_verified_current_source_location() {
    let workspace = tempfile::tempdir().expect("independent navigation workspace");
    write_test_config(workspace.path());
    fs::create_dir(workspace.path().join("src")).expect("source directory");
    let source =
        "// packet fixture\n\npub fn checksum(data: &[u8]) -> usize {\n    data.len()\n}\n";
    fs::write(workspace.path().join("src/packet.rs"), source).expect("source fixture");
    let symbols = aether_parse::SymbolExtractor::new()
        .expect("parser")
        .extract_from_path(Path::new("src/packet.rs"), source)
        .expect("parse source");
    let symbol = symbols
        .iter()
        .find(|item| item.name == "checksum")
        .expect("checksum");
    let store = SqliteStore::open(workspace.path()).expect("synthetic catalog");
    store
        .upsert_symbol(aether_store::SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: "rust".into(),
            kind: "function".into(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 0,
        })
        .expect("index independent symbol");
    let intent = super::EnhanceIntent {
        target_symbols: vec![symbol.qualified_name.clone()],
        ..Default::default()
    };
    let assembled = super::assemble_context(workspace.path(), &store, &intent)
        .expect("assemble source context");
    let rendered = crate::enhance_templates::render_enhanced_document(
        "Locate checksum",
        &assembled.context,
        512,
        false,
    )
    .expect("render context");
    assert!(rendered.contains("src/packet.rs:3-5"));
}

fn seed_symbol(
    store: &SqliteStore,
    id: &str,
    file_path: &str,
    qualified_name: &str,
    kind: &str,
    intent: &str,
) {
    store
        .upsert_symbol(aether_store::SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: kind.to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        })
        .expect("upsert symbol");
    let sir_json = serde_json::json!({
        "intent": intent,
        "inputs": ["request"],
        "outputs": ["result"],
        "side_effects": ["writes state"],
        "dependencies": ["db"],
        "error_modes": ["io failure"],
        "confidence": 0.81
    })
    .to_string();
    store
        .write_sir_blob(id, sir_json.as_str())
        .expect("write sir");
    store
        .upsert_sir_meta(SirMetaRecord {
            id: id.to_owned(),
            sir_hash: format!("hash-{id}"),
            sir_version: 1,
            provider: "mock".to_owned(),
            model: "mock".to_owned(),
            generation_pass: "triage".to_owned(),
            reasoning_trace: Some("mock reasoning".to_owned()),
            prompt_hash: None,
            staleness_score: None,
            updated_at: 1_700_000_100,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: 1_700_000_100,
        })
        .expect("upsert meta");
}

fn seed_workspace(workspace: &Path) -> SqliteStore {
    write_test_config(workspace);
    fs::create_dir_all(workspace.join("src")).expect("mkdir src");
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn login() {}\npub fn auth_helper() {}\n",
    )
    .expect("write lib");
    let store = SqliteStore::open(workspace).expect("open store");
    seed_symbol(
        &store,
        "sym_login",
        "src/lib.rs",
        "demo::login",
        "function",
        "Handles login requests and validates credentials.",
    );
    seed_symbol(
        &store,
        "sym_helper",
        "src/lib.rs",
        "demo::auth_helper",
        "function",
        "Provides helper routines for authentication state.",
    );
    store
        .upsert_edges(&[
            SymbolEdge {
                target_symbol_id: Some("sym_helper".to_owned()),
                target_file_path: None,
                source_id: "sym_login".to_owned(),
                target_qualified_name: "demo::auth_helper".to_owned(),
                edge_kind: EdgeKind::DependsOn,
                file_path: "src/lib.rs".to_owned(),
            },
            SymbolEdge {
                target_symbol_id: Some("sym_login".to_owned()),
                target_file_path: None,
                source_id: "sym_helper".to_owned(),
                target_qualified_name: "demo::login".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
        ])
        .expect("upsert edges");
    store
}

#[test]
fn keyword_extraction_classifies_bug_fix_and_preserves_login_concept() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_test_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    let intent =
        extract_intent_via_keywords(workspace, &store, "fix the login bug").expect("intent");

    assert_eq!(intent.task_type, TaskType::BugFix);
    assert!(intent.concepts.iter().any(|concept| concept == "login"));
}

#[test]
fn qualified_method_prompt_resolves_edit_sites_instead_of_task_verbs() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = seed_qualified_workspace(workspace);
    let result = enhance_prompt_core(
            workspace,
            &store,
            &EnhanceRequest::new(
                "Add deserializer_kwargs to Serializer.__init__ and forward it in Serializer.load_payload while keeping Serializer.dump_payload unchanged.",
                1_600,
                false,
                true,
            ),
        )
        .expect("enhance qualified prompt");

    for method in ["__init__", "load_payload", "dump_payload"] {
        assert!(
            result
                .resolved_symbols
                .contains(&format!("itsdangerous.serializer::Serializer::{method}"))
        );
    }
    assert!(
        result
            .resolved_symbols
            .iter()
            .all(|name| !name.contains("BadData"))
    );
    assert!(result.enhanced_prompt.contains("Loads a payload"));
}

fn seed_qualified_workspace(workspace: &Path) -> SqliteStore {
    write_test_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    for (id, name, intent) in [
        (
            "init",
            "itsdangerous.serializer::Serializer::__init__",
            "Configures serializers",
        ),
        (
            "load",
            "itsdangerous.serializer::Serializer::load_payload",
            "Loads a payload",
        ),
        (
            "dump",
            "itsdangerous.serializer::Serializer::dump_payload",
            "Dumps a payload",
        ),
        (
            "other",
            "itsdangerous.serializer::OtherSerializer::load_payload",
            "Unrelated owner",
        ),
        (
            "extension",
            "itsdangerous.serializer::Serializer::load_payload_extra",
            "Unrelated method",
        ),
        ("bad", "itsdangerous.exc::BadData", "Unrelated error"),
    ] {
        seed_symbol(&store, id, "serializer.py", name, "method", intent);
    }
    store
}

#[test]
fn qualified_resolution_accepts_dots_and_scopes_without_losing_owner() {
    let temp = tempdir().expect("tempdir");
    let store = seed_qualified_workspace(temp.path());
    for query in [
        "Serializer.load_payload",
        "Serializer::load_payload",
        "itsdangerous.serializer.Serializer.load_payload",
        "itsdangerous.serializer::Serializer::load_payload",
        "load",
    ] {
        let records = resolve_symbol_query(&store, query, 3).expect("resolve qualified symbol");
        assert_eq!(records.len(), 1, "query {query}");
        assert_eq!(records[0].id, "load", "query {query}");
    }
    assert!(
        resolve_symbol_query(&store, "MissingSerializer.load_payload", 3)
            .expect("resolve missing owner")
            .is_empty()
    );
}

#[test]
fn unresolved_qualified_method_is_reported_without_fuzzy_substitution() {
    let temp = tempdir().expect("tempdir");
    let store = seed_qualified_workspace(temp.path());
    let result = enhance_prompt_core(
        temp.path(),
        &store,
        &EnhanceRequest::new("MissingSerializer.load_payload", 1_600, false, true),
    )
    .expect("enhance missing owner");
    assert!(result.resolved_symbols.is_empty());
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| { warning == "no symbol matched 'MissingSerializer.load_payload'" })
    );
}

#[test]
fn dotted_method_is_an_explicit_target_without_requiring_offline_flag() {
    assert!(prompt_has_explicit_targets(
        "Inspect Serializer.load_payload."
    ));
    assert!(!prompt_has_explicit_targets("Inspect the serializer."));
    assert!(!prompt_has_explicit_targets(
        "Investigate v1.2 compatibility"
    ));
    assert!(!prompt_has_explicit_targets(
        "Inspect Serializer..load_payload"
    ));
}

#[test]
fn task_words_preserve_real_symbol_components() {
    let temp = tempdir().expect("tempdir");
    let store = seed_qualified_workspace(temp.path());
    for (id, name) in [("write", "std::io::Write"), ("add", "math::add")] {
        seed_symbol(&store, id, "lib.rs", name, "function", "Real named symbol");
    }
    let result = enhance_prompt_core(
        temp.path(),
        &store,
        &EnhanceRequest::new("Inspect Write and add", 1_600, false, true),
    )
    .expect("enhance task-word symbols");
    assert!(
        result
            .resolved_symbols
            .contains(&"std::io::Write".to_owned())
    );
    assert!(result.resolved_symbols.contains(&"math::add".to_owned()));
    assert!(
        result
            .resolved_symbols
            .iter()
            .all(|name| !name.contains("BadData"))
    );
}

#[test]
fn template_output_contains_expected_sections() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = seed_workspace(workspace);
    let result = enhance_prompt_core(
        workspace,
        &store,
        &EnhanceRequest {
            prompt: "fix the login bug".to_owned(),
            budget: 8_000,
            rewrite: false,
            offline: true,
            format: EnhanceDocumentFormat::Text,
        },
    )
    .expect("enhance");

    assert!(result.enhanced_prompt.contains("## Enhanced Prompt"));
    assert!(result.enhanced_prompt.contains("### Target Symbols"));
    assert!(result.enhanced_prompt.contains("### Related Files"));
}

#[test]
fn qualified_miss_suggests_indexed_owner_without_resolving_it() {
    let temp = tempdir().expect("independent suggestion catalog");
    write_test_config(temp.path());
    let store = SqliteStore::open(temp.path()).expect("catalog");
    seed_symbol(
        &store,
        "hint",
        "src/quasar/packet.py",
        "quasar.packet::PacketEmitter::emit_packet",
        "method",
        "Independent fixture.",
    );
    let intent = super::EnhanceIntent {
        target_symbols: vec!["packet.emitPacket".into()],
        ..Default::default()
    };
    let result = super::assemble_context(temp.path(), &store, &intent).expect("assemble");
    assert!(result.resolved_symbols.is_empty());
    assert!(result.referenced_files.is_empty());
    assert!(result.context.symbols.is_empty());
    assert!(result.warnings.iter().any(|warning| {
        warning.contains("suggestions only; not resolved")
            && warning.contains("quasar.packet::PacketEmitter::emit_packet")
            && warning.contains("src/quasar/packet.py")
    }));
    assert!(
        resolve_symbol_query(&store, "packet.emitPacket", 3)
            .expect("exact resolver")
            .is_empty()
    );
}

#[test]
fn exact_owner_match_does_not_add_spelling_suggestions() {
    let temp = tempdir().expect("independent exact catalog");
    write_test_config(temp.path());
    let store = SqliteStore::open(temp.path()).expect("catalog");
    seed_symbol(
        &store,
        "exact",
        "src/quasar/packet.py",
        "quasar.packet::PacketEmitter::emitPacket",
        "method",
        "Exact fixture.",
    );
    seed_symbol(
        &store,
        "hint",
        "src/quasar/packet.py",
        "quasar.packet::PacketEmitter::emit_packet",
        "method",
        "Spelling fixture.",
    );
    let intent = super::EnhanceIntent {
        target_symbols: vec!["PacketEmitter.emitPacket".into()],
        ..Default::default()
    };
    let result = super::assemble_context(temp.path(), &store, &intent).expect("assemble");
    assert_eq!(
        result.resolved_symbols,
        vec!["quasar.packet::PacketEmitter::emitPacket"]
    );
    assert!(
        !result
            .warnings
            .iter()
            .any(|warning| warning.contains("suggestions only"))
    );
}

#[test]
fn empty_workspace_returns_original_prompt_with_warning() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_test_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    let result = enhance_prompt_core(
        workspace,
        &store,
        &EnhanceRequest {
            prompt: "investigate auth".to_owned(),
            budget: 8_000,
            rewrite: false,
            offline: true,
            format: EnhanceDocumentFormat::Text,
        },
    )
    .expect("enhance");

    assert_eq!(result.enhanced_prompt, "investigate auth");
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("contains no symbols"))
    );
}

#[test]
fn token_budget_truncates_large_context() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = seed_workspace(workspace);
    seed_symbol(
        &store,
        "sym_extra",
        "src/lib.rs",
        "demo::extra_long_context",
        "function",
        "Produces a very long narrative intent. Produces a very long narrative intent. Produces a very long narrative intent. Produces a very long narrative intent. Produces a very long narrative intent.",
    );
    let result = enhance_prompt_core(
        workspace,
        &store,
        &EnhanceRequest {
            prompt: "refactor login and auth helper".to_owned(),
            budget: 120,
            rewrite: false,
            offline: true,
            format: EnhanceDocumentFormat::Text,
        },
    )
    .expect("enhance");

    assert!(result.token_count <= 120);
    assert!(result.enhanced_prompt.contains("[... truncated ...]"));
}

#[test]
fn read_sir_annotation_returns_none_and_logs_on_parse_error() {
    let temp = tempdir().expect("tempdir");
    write_test_config(temp.path());
    let store = SqliteStore::open(temp.path()).expect("open store");
    store
        .write_sir_blob("sym-invalid", "{invalid")
        .expect("write invalid sir");

    let (sir, logs) = capture_logs(|| read_sir_annotation(&store, "sym-invalid"));

    assert!(sir.is_none());
    assert!(logs.contains("failed to parse SIR blob for enhancement context"));
    assert!(logs.contains("sym-invalid"));
}
