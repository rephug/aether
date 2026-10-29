//! Small provider-free storage diagnostic. All data is synthetic and disposable.
//! Run only with CARGO_TARGET_DIR/TMPDIR pointing outside the repository.

#![recursion_limit = "256"]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use aether_config::{AetherConfig, EmbeddingVectorBackend, GraphBackend, save_workspace_config};
use aether_core::content_hash;
use aether_store::{
    LanceVectorStore, SqliteStore, SqliteVectorStore, SymbolRecord, VectorRecord,
    VectorSearchResult, VectorStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

type DiagnosticResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PROTOCOL_JSON: &str =
    include_str!("../../../docs/benchmarks/vector-storage-diagnostic-protocol-v1.json");
const PROVIDER: &str = "deterministic-storage-fixture";
const MODEL: &str = "synthetic-3072-storage-fixture-v1";

#[derive(Clone, Deserialize, Serialize)]
struct Protocol {
    protocol_version: String,
    seed: u64,
    rows: usize,
    dimensions: usize,
    queries: usize,
    k: u32,
    mixed_readers: usize,
    writer_updates: usize,
    tokio_workers: usize,
    backend_order: Vec<String>,
}

#[derive(Serialize)]
struct QueryReceipt {
    reader: usize,
    query: usize,
    start_offset_ms: f64,
    api_ms: f64,
    result_ids: Vec<String>,
    scores: Vec<f32>,
    recall_at_k: f64,
    ordered_exact_match: bool,
    duplicate_ids: bool,
    error: Option<String>,
}

#[derive(Serialize)]
struct WriteReceipt {
    index: usize,
    start_offset_ms: f64,
    api_ms: f64,
    error: Option<String>,
}

struct Fixture {
    rows: Vec<VectorRecord>,
    queries: Vec<Vec<f32>>,
    gold: Vec<Vec<String>>,
    dataset_hash: String,
    query_hash: String,
    oracle_hash: String,
    min_oracle_boundary_gap: f64,
    generation_ms: f64,
    oracle_ms: f64,
}

fn contextual<T, E: std::fmt::Display>(value: Result<T, E>, stage: &str) -> DiagnosticResult<T> {
    value.map_err(|error| format!("{stage}: {error}").into())
}

fn milliseconds(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

fn scratch_root() -> DiagnosticResult<PathBuf> {
    let target =
        PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").ok_or("CARGO_TARGET_DIR is required")?);
    let scratch = PathBuf::from(std::env::var_os("TMPDIR").ok_or("TMPDIR is required")?);
    let target = contextual(target.canonicalize(), "resolve build target")?;
    let scratch = contextual(scratch.canonicalize(), "resolve build TMPDIR")?;
    let current = contextual(std::env::current_dir(), "current directory")?;
    if !scratch.starts_with(&target)
        || target.starts_with(current)
        || scratch.starts_with("/tmp")
        || scratch.starts_with("/mnt")
        || scratch
            .components()
            .any(|part| part.as_os_str() == ".aether")
    {
        return Err(
            "TMPDIR must be inside the external build target, outside /tmp, /mnt and active stores"
                .into(),
        );
    }
    Ok(scratch)
}

fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn vector(state: &mut u64, dimensions: usize) -> Vec<f32> {
    let mut values = (0..dimensions)
        .map(|_| {
            let bits = (next_random(state) >> 32) as u32;
            ((f64::from(bits) / f64::from(u32::MAX)) * 2.0 - 1.0) as f32
        })
        .collect::<Vec<_>>();
    let norm = values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    for value in &mut values {
        *value = (f64::from(*value) / norm) as f32;
    }
    values
}

fn vector_hash(previous: &str, id: &str, values: &[f32]) -> DiagnosticResult<String> {
    let mut text = format!("{previous}\n{id}\n");
    for value in values {
        contextual(
            write!(&mut text, "{:08x}", value.to_bits()),
            "encode vector identity",
        )?;
    }
    Ok(content_hash(&text))
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    let dot = left
        .iter()
        .zip(right)
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum::<f64>();
    let left_norm = left
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    let right_norm = right
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    dot / (left_norm * right_norm)
}

fn fixture(protocol: &Protocol) -> DiagnosticResult<Fixture> {
    let started = Instant::now();
    let mut state = protocol.seed;
    let mut dataset_hash = content_hash("float32-bits-chain-v1");
    let mut rows = Vec::with_capacity(protocol.rows);
    for index in 0..protocol.rows {
        let symbol_id = format!("synthetic-{index:05}");
        let embedding = vector(&mut state, protocol.dimensions);
        dataset_hash = vector_hash(&dataset_hash, &symbol_id, &embedding)?;
        rows.push(VectorRecord {
            symbol_id,
            sir_hash: format!("synthetic-hash-{index}"),
            provider: PROVIDER.to_owned(),
            model: MODEL.to_owned(),
            embedding,
            updated_at: 1,
        });
    }
    let mut query_hash = content_hash("query-float32-bits-chain-v1");
    let mut queries = Vec::with_capacity(protocol.queries);
    for index in 0..protocol.queries {
        let query = vector(&mut state, protocol.dimensions);
        query_hash = vector_hash(&query_hash, &format!("query-{index:02}"), &query)?;
        queries.push(query);
    }
    let generation_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    let mut gold = Vec::with_capacity(protocol.queries);
    let mut min_gap = f64::INFINITY;
    for query in &queries {
        let mut ranked = rows
            .iter()
            .map(|row| (cosine(&row.embedding, query), row.symbol_id.clone()))
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        let k = protocol.k as usize;
        min_gap = min_gap.min(ranked[k - 1].0 - ranked[k].0);
        gold.push(
            ranked
                .into_iter()
                .take(k)
                .map(|(_, id)| id)
                .collect::<Vec<_>>(),
        );
    }
    let oracle_ms = milliseconds(started.elapsed());
    let oracle_hash = content_hash(&contextual(
        serde_json::to_string(&gold),
        "encode oracle identity",
    )?);
    Ok(Fixture {
        rows,
        queries,
        gold,
        dataset_hash,
        query_hash,
        oracle_hash,
        min_oracle_boundary_gap: min_gap,
        generation_ms,
        oracle_ms,
    })
}

fn open_backend(
    backend: &str,
    workspace: &Path,
    runtime: &Runtime,
) -> DiagnosticResult<Arc<dyn VectorStore>> {
    match backend {
        "sqlite" => Ok(Arc::new(contextual(
            SqliteVectorStore::new(workspace),
            "open SQLite vector API",
        )?)),
        "lance" => Ok(Arc::new(contextual(
            runtime.block_on(LanceVectorStore::open(workspace)),
            "open Lance vector API",
        )?)),
        _ => Err(format!("unknown diagnostic backend: {backend}").into()),
    }
}

fn measure_query(
    store: &dyn VectorStore,
    runtime: &Runtime,
    fixture: &Fixture,
    protocol: &Protocol,
    reader: usize,
    query: usize,
    phase_start: Instant,
) -> QueryReceipt {
    let start_offset_ms = milliseconds(phase_start.elapsed());
    let started = Instant::now();
    let result = runtime.block_on(store.search_nearest(
        &fixture.queries[query],
        PROVIDER,
        MODEL,
        protocol.k,
    ));
    let api_ms = milliseconds(started.elapsed());
    let (rows, error): (Vec<VectorSearchResult>, _) = match result {
        Ok(rows) => (rows, None),
        Err(error) => (
            Vec::new(),
            Some(error.to_string().chars().take(2000).collect()),
        ),
    };
    let result_ids = rows
        .iter()
        .map(|row| row.symbol_id.clone())
        .collect::<Vec<_>>();
    let unique = result_ids.iter().collect::<BTreeSet<_>>();
    let gold = fixture.gold[query].iter().collect::<BTreeSet<_>>();
    let recall_at_k = unique.intersection(&gold).count() as f64 / f64::from(protocol.k);
    QueryReceipt {
        reader,
        query,
        start_offset_ms,
        api_ms,
        duplicate_ids: unique.len() != result_ids.len(),
        scores: rows.iter().map(|row| row.semantic_score).collect(),
        ordered_exact_match: result_ids == fixture.gold[query],
        result_ids,
        recall_at_k,
        error,
    }
}

fn latency(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({"count":0});
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let quantile =
        |percent: usize| sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)];
    json!({"count":sorted.len(),"p50_ms":quantile(50),"p95_ms":quantile(95),"p99_ms":quantile(99),"max_ms":sorted[sorted.len()-1]})
}

fn query_summary(receipts: &[QueryReceipt], expected: usize, k: u32, wall_ms: f64) -> Value {
    let mut identities = BTreeMap::new();
    for receipt in receipts {
        *identities
            .entry((receipt.reader, receipt.query))
            .or_insert(0) += 1;
    }
    let unique_observation =
        |receipt: &QueryReceipt| identities.get(&(receipt.reader, receipt.query)) == Some(&1);
    let failures = receipts
        .iter()
        .filter(|receipt| receipt.error.is_some())
        .count();
    let recall_sum = receipts
        .iter()
        .filter(|receipt| unique_observation(receipt) && receipt.error.is_none())
        .map(|receipt| {
            if receipt.recall_at_k.is_finite() && (0.0..=1.0).contains(&receipt.recall_at_k) {
                receipt.recall_at_k
            } else {
                0.0
            }
        })
        .sum::<f64>();
    let duplicate_id_queries = receipts
        .iter()
        .filter(|receipt| {
            receipt.duplicate_ids
                || receipt.result_ids.iter().collect::<BTreeSet<_>>().len()
                    != receipt.result_ids.len()
        })
        .count();
    let correct = expected > 0
        && receipts.len() == expected
        && identities.len() == receipts.len()
        && duplicate_id_queries == 0
        && receipts.iter().all(|receipt| {
            receipt.error.is_none()
                && receipt.result_ids.len() == k as usize
                && receipt.recall_at_k == 1.0
                && receipt.scores.len() == receipt.result_ids.len()
                && receipt.scores.iter().all(|score| score.is_finite())
        });
    let denominator = expected.max(1) as f64;
    json!({"expected_queries":expected,"recorded_queries":receipts.len(),"unique_query_receipts":identities.len(),
        "missing_query_receipts":expected.saturating_sub(identities.len()),"duplicate_query_receipts":receipts.len().saturating_sub(identities.len()),
        "api_failures":failures,"duplicate_id_queries":duplicate_id_queries,
        "mean_recall_at_k_with_failures_in_denominator":recall_sum / denominator,
        "ordered_exact_match_fraction":receipts.iter().filter(|receipt|unique_observation(receipt) && receipt.error.is_none() && receipt.ordered_exact_match).count() as f64 / denominator,
        "exact_recall_gate_passed":correct,"api_latency":latency(&receipts.iter().map(|receipt|receipt.api_ms).collect::<Vec<_>>()),
        "wall_ms_including_driver_validation":wall_ms,"reader_queries_per_second":receipts.len() as f64 / (wall_ms/1000.0)})
}

fn payload_summary(
    expected: &[VectorRecord],
    updated_rows: usize,
    observed: Result<Vec<VectorRecord>, String>,
) -> Value {
    let (rows, api_error) = match observed {
        Ok(rows) => (rows, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    let expected_by_id = expected
        .iter()
        .enumerate()
        .map(|(index, row)| (row.symbol_id.as_str(), (index, row)))
        .collect::<BTreeMap<_, _>>();
    let mut rows_by_id: BTreeMap<&str, Vec<&VectorRecord>> = BTreeMap::new();
    for row in &rows {
        rows_by_id.entry(&row.symbol_id).or_default().push(row);
    }
    let matches = |row: &VectorRecord, original: &VectorRecord, index: usize| {
        row.provider == original.provider
            && row.model == original.model
            && if index < updated_rows {
                row.sir_hash == format!("rewrite-{index}") && row.updated_at == 2
            } else {
                row.sir_hash == original.sir_hash && row.updated_at == original.updated_at
            }
            && row.embedding.len() == original.embedding.len()
            && row
                .embedding
                .iter()
                .zip(&original.embedding)
                .all(|(left, right)| left.to_bits() == right.to_bits())
    };
    let missing_ids = expected_by_id
        .keys()
        .filter(|id| !rows_by_id.contains_key(**id))
        .copied()
        .collect::<Vec<_>>();
    let unexpected_ids = rows_by_id
        .keys()
        .filter(|id| !expected_by_id.contains_key(**id))
        .copied()
        .collect::<Vec<_>>();
    let duplicate_ids = rows_by_id
        .iter()
        .filter_map(|(id, rows)| (rows.len() != 1).then_some(*id))
        .collect::<Vec<_>>();
    let mismatched_ids = rows_by_id
        .iter()
        .filter_map(|(id, rows)| {
            expected_by_id.get(id).and_then(|(index, original)| {
                rows.iter()
                    .any(|row| !matches(row, original, *index))
                    .then_some(*id)
            })
        })
        .collect::<Vec<_>>();
    let matched_rows = expected_by_id
        .iter()
        .filter(|(id, (index, original))| {
            rows_by_id
                .get(*id)
                .is_some_and(|rows| rows.len() == 1 && matches(rows[0], original, *index))
        })
        .count();
    let passed = !expected.is_empty()
        && expected_by_id.len() == expected.len()
        && updated_rows <= expected.len()
        && api_error.is_none()
        && rows.len() == expected.len()
        && matched_rows == expected.len();
    json!({"expected_rows":expected.len(),"expected_changed_rows":updated_rows,
        "expected_unchanged_rows":expected.len().saturating_sub(updated_rows),"recorded_rows":rows.len(),"matched_rows":matched_rows,
        "missing_ids":missing_ids,"unexpected_ids":unexpected_ids,"duplicate_ids":duplicate_ids,"mismatched_ids":mismatched_ids,
        "api_error":api_error,"exact_rows_fraction_with_failures_in_denominator":matched_rows as f64 / expected.len().max(1) as f64,
        "payloads_and_metadata_gate_passed":passed})
}

fn validate_payloads(
    store: &dyn VectorStore,
    runtime: &Runtime,
    expected: &[VectorRecord],
    updated_rows: usize,
) -> Value {
    let phase_start = Instant::now();
    let ids = expected
        .iter()
        .map(|row| row.symbol_id.clone())
        .collect::<Vec<_>>();
    let id_copy_ms = milliseconds(phase_start.elapsed());
    let started = Instant::now();
    let observed = runtime
        .block_on(store.list_embeddings_for_symbols(PROVIDER, MODEL, &ids))
        .map_err(|error| error.to_string().chars().take(2000).collect());
    let read_api_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    let mut summary = payload_summary(expected, updated_rows, observed);
    summary["comparison_ms"] = json!(milliseconds(started.elapsed()));
    summary["read_api_ms"] = json!(read_api_ms);
    summary["requested_id_copy_ms"] = json!(id_copy_ms);
    summary["wall_ms_including_driver_validation"] = json!(milliseconds(phase_start.elapsed()));
    summary
}

fn memory() -> Value {
    let text = fs::read_to_string("/proc/self/status").ok();
    let field = |name: &str| {
        text.as_ref()
            .and_then(|text| text.lines().find(|line| line.starts_with(name)))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u64>().ok())
    };
    json!({"rss_kib":field("VmRSS:"),"process_peak_rss_kib":field("VmHWM:"),
        "scope":"whole arm child including driver/corpus/allocators; not isolated engine memory"})
}

fn disk(workspace: &Path) -> DiagnosticResult<Value> {
    fn walk(
        path: &Path,
        logical: &mut u64,
        allocated: &mut u64,
        files: &mut usize,
    ) -> DiagnosticResult<()> {
        for entry in contextual(fs::read_dir(path), "read diagnostic disk inventory")? {
            let entry = contextual(entry, "read inventory entry")?;
            let metadata = contextual(entry.metadata(), "read inventory metadata")?;
            if metadata.is_dir() {
                walk(&entry.path(), logical, allocated, files)?;
            } else if metadata.is_file() {
                *logical += metadata.len();
                *files += 1;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    *allocated += metadata.blocks() * 512;
                }
            }
        }
        Ok(())
    }
    let (mut logical, mut allocated, mut files) = (0, 0, 0);
    walk(workspace, &mut logical, &mut allocated, &mut files)?;
    Ok(
        json!({"logical_bytes":logical,"allocated_bytes":if cfg!(unix){Some(allocated)}else{None},"files":files}),
    )
}

fn mixed_phase(
    backend: &str,
    workspace: &Path,
    runtime: Arc<Runtime>,
    fixture: Arc<Fixture>,
    protocol: Arc<Protocol>,
) -> DiagnosticResult<Value> {
    let opened = Instant::now();
    let readers = (0..protocol.mixed_readers)
        .map(|_| open_backend(backend, workspace, &runtime))
        .collect::<DiagnosticResult<Vec<_>>>()?;
    let writer = open_backend(backend, workspace, &runtime)?;
    let handle_open_ms = milliseconds(opened.elapsed());
    let barrier = Arc::new(Barrier::new(protocol.mixed_readers + 2));
    let phase_start = Instant::now();
    let mut handles = Vec::new();
    for (reader, store) in readers.into_iter().enumerate() {
        let (runtime, fixture, protocol, barrier) = (
            Arc::clone(&runtime),
            Arc::clone(&fixture),
            Arc::clone(&protocol),
            Arc::clone(&barrier),
        );
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            (0..protocol.queries)
                .map(|query| {
                    measure_query(
                        store.as_ref(),
                        &runtime,
                        &fixture,
                        &protocol,
                        reader,
                        query,
                        phase_start,
                    )
                })
                .collect::<Vec<_>>()
        }));
    }
    let (writer_runtime, writer_fixture, writer_protocol, writer_barrier) = (
        Arc::clone(&runtime),
        Arc::clone(&fixture),
        Arc::clone(&protocol),
        Arc::clone(&barrier),
    );
    let writer_handle = std::thread::spawn(move || {
        writer_barrier.wait();
        let mut writes = Vec::new();
        for index in 0..writer_protocol.writer_updates {
            let mut record = writer_fixture.rows[index].clone();
            record.sir_hash = format!("rewrite-{index}");
            record.updated_at = 2;
            let start_offset_ms = milliseconds(phase_start.elapsed());
            let started = Instant::now();
            let result = writer_runtime.block_on(writer.upsert_embedding(record));
            writes.push(WriteReceipt {
                index,
                start_offset_ms,
                api_ms: milliseconds(started.elapsed()),
                error: result
                    .err()
                    .map(|error| error.to_string().chars().take(2000).collect()),
            });
        }
        writes
    });
    barrier.wait();
    let mut receipts = Vec::new();
    for handle in handles {
        receipts.extend(handle.join().map_err(|_| "diagnostic reader panicked")?);
    }
    let writes = writer_handle
        .join()
        .map_err(|_| "diagnostic writer panicked")?;
    let wall_ms = milliseconds(phase_start.elapsed());
    receipts.sort_by_key(|receipt| (receipt.reader, receipt.query));
    let writer_start = writes.first().map_or(0.0, |write| write.start_offset_ms);
    let writer_end = writes
        .last()
        .map_or(0.0, |write| write.start_offset_ms + write.api_ms);
    let overlap = receipts
        .iter()
        .filter(|receipt| {
            receipt.start_offset_ms < writer_end
                && receipt.start_offset_ms + receipt.api_ms > writer_start
        })
        .count();
    let summary = query_summary(
        &receipts,
        protocol.mixed_readers * protocol.queries,
        protocol.k,
        wall_ms,
    );
    Ok(
        json!({"reader_handles":protocol.mixed_readers,"writer_handles":1,
        "handle_open_ms":handle_open_ms,"query_summary":summary,"query_receipts":receipts,
        "write_receipts":writes,"write_latency":latency(&writes.iter().map(|write|write.api_ms).collect::<Vec<_>>()),
        "queries_overlapping_writer_burst":overlap,"writer_is_bounded_burst":true,
        "memory_after_phase":memory(),"disk_with_no_reader_writer_handles":disk(workspace)?}),
    )
}

fn run_arm(backend: &str, protocol: Arc<Protocol>) -> DiagnosticResult<Value> {
    let total_started = Instant::now();
    let scratch = scratch_root()?;
    let workspace = contextual(
        tempfile::Builder::new()
            .prefix("aether-vector-diagnostic-")
            .tempdir_in(&scratch),
        "create isolated fixture",
    )?;
    let fixture = Arc::new(fixture(&protocol)?);
    let started = Instant::now();
    let runtime = Arc::new(contextual(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(protocol.tokio_workers)
            .enable_all()
            .build(),
        "create diagnostic runtime",
    )?);
    let runtime_init_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.storage.mirror_sir_files = false;
    config.embeddings.enabled = false;
    config.embeddings.vector_backend = if backend == "sqlite" {
        EmbeddingVectorBackend::Sqlite
    } else {
        EmbeddingVectorBackend::Lancedb
    };
    contextual(
        save_workspace_config(workspace.path(), &config),
        "save isolated config",
    )?;
    let config_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    // Read only the freshly saved defaults above. Do not load user configuration
    // or expand environment-backed credential names into values.
    let saved_config_toml = contextual(
        fs::read_to_string(workspace.path().join(".aether/config.toml")),
        "read fresh diagnostic config snapshot",
    )?;
    let saved_config_hash = content_hash(&saved_config_toml);
    let config_snapshot_read_and_hash_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    let catalog = contextual(
        SqliteStore::open(workspace.path()),
        "initialize shared SQLite catalog",
    )?;
    let records = fixture
        .rows
        .iter()
        .map(|row| SymbolRecord {
            id: row.symbol_id.clone(),
            file_path: format!("synthetic/{}.rs", row.symbol_id),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: row.symbol_id.clone(),
            signature_fingerprint: row.symbol_id.clone(),
            last_seen_at: 1,
        })
        .collect::<Vec<_>>();
    contextual(
        catalog.upsert_symbols(&records),
        "seed equal symbol catalogs",
    )?;
    drop(catalog);
    let catalog_setup_ms = milliseconds(started.elapsed());
    let baseline_memory = memory();
    let started = Instant::now();
    let store = open_backend(backend, workspace.path(), &runtime)?;
    let empty_open_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    let rows = fixture.rows.clone();
    let caller_record_copy_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    contextual(
        runtime.block_on(store.upsert_embedding_batch(rows)),
        "upsert equal generated vectors",
    )?;
    let upsert_batch_ms = milliseconds(started.elapsed());
    let initial_payload_validation = validate_payloads(store.as_ref(), &runtime, &fixture.rows, 0);
    let disk_after_upsert = disk(workspace.path())?;
    let memory_after_upsert = memory();
    let started = Instant::now();
    drop(store);
    let initial_close_ms = milliseconds(started.elapsed());
    let started = Instant::now();
    let store = open_backend(backend, workspace.path(), &runtime)?;
    let reopen_ms = milliseconds(started.elapsed());
    let phase_start = Instant::now();
    let first_after_reopen = measure_query(
        store.as_ref(),
        &runtime,
        &fixture,
        &protocol,
        0,
        0,
        phase_start,
    );
    let phase_start = Instant::now();
    let warm = (0..protocol.queries)
        .map(|query| {
            measure_query(
                store.as_ref(),
                &runtime,
                &fixture,
                &protocol,
                0,
                query,
                phase_start,
            )
        })
        .collect::<Vec<_>>();
    let warm_wall_ms = milliseconds(phase_start.elapsed());
    let single_summary = query_summary(&warm, protocol.queries, protocol.k, warm_wall_ms);
    let started = Instant::now();
    drop(store);
    let single_reader_close_ms = milliseconds(started.elapsed());
    let mixed = mixed_phase(
        backend,
        workspace.path(),
        Arc::clone(&runtime),
        Arc::clone(&fixture),
        Arc::clone(&protocol),
    )?;
    let started = Instant::now();
    let check = open_backend(backend, workspace.path(), &runtime)?;
    let validation_open_ms = milliseconds(started.elapsed());
    let post_update_validation = validate_payloads(
        check.as_ref(),
        &runtime,
        &fixture.rows,
        protocol.writer_updates,
    );
    let payloads_match = post_update_validation["payloads_and_metadata_gate_passed"] == true;
    let started = Instant::now();
    drop(check);
    let validation_close_ms = milliseconds(started.elapsed());
    let passed = initial_payload_validation["payloads_and_metadata_gate_passed"] == true
        && first_after_reopen.error.is_none()
        && first_after_reopen.recall_at_k == 1.0
        && first_after_reopen.result_ids.len() == protocol.k as usize
        && !first_after_reopen.duplicate_ids
        && first_after_reopen
            .scores
            .iter()
            .all(|score| score.is_finite())
        && single_summary["exact_recall_gate_passed"] == true
        && mixed["query_summary"]["exact_recall_gate_passed"] == true
        && mixed["write_receipts"].as_array().is_some_and(|writes| {
            writes.len() == protocol.writer_updates
                && writes.iter().all(|write| write["error"].is_null())
        })
        && payloads_match;
    let started = Instant::now();
    drop(runtime);
    let runtime_shutdown_ms = milliseconds(started.elapsed());
    Ok(
        json!({"backend":backend,"passed":passed,"dataset_hash":fixture.dataset_hash,"query_hash":fixture.query_hash,
        "oracle_hash":fixture.oracle_hash,"hash_definition":"iterated BLAKE3 over IDs and float32-bit strings; oracle canonical JSON",
        "oracle_ids":fixture.gold,"oracle_min_k_boundary_gap_f64":fixture.min_oracle_boundary_gap,
        "dataset_generation_and_hash_ms":fixture.generation_ms,"oracle_generation_ms":fixture.oracle_ms,
        "runtime_init_ms":runtime_init_ms,"config_write_ms":config_ms,"config_snapshot_read_and_hash_ms":config_snapshot_read_and_hash_ms,
        "saved_config_toml":saved_config_toml,"config_hash":saved_config_hash,"config_hash_algorithm":"BLAKE3 of actual saved TOML UTF-8 bytes",
        "config_source":"fresh AetherConfig defaults with diagnostic-only storage/embedding settings; no environment credential expansion","shared_catalog_setup_ms":catalog_setup_ms,
        "empty_vector_api_open_ms":empty_open_ms,"caller_record_copy_ms":caller_record_copy_ms,
        "public_upsert_batch_ms":upsert_batch_ms,"initial_full_row_validation":initial_payload_validation,"initial_close_ms":initial_close_ms,"warm_process_reopen_ms":reopen_ms,
        "expected_search_api_calls":1+protocol.queries+protocol.mixed_readers*protocol.queries,
        "first_query_after_reopen":first_after_reopen,"one_reader_summary":single_summary,"one_reader_receipts":warm,
        "single_reader_close_ms":single_reader_close_ms,"mixed_phase":mixed,"post_update_validation_open_ms":validation_open_ms,"post_update_full_row_validation":post_update_validation,
        "validation_close_ms":validation_close_ms,"runtime_shutdown_ms":runtime_shutdown_ms,"post_update_payloads_and_metadata_match":payloads_match,"disk_after_upsert_with_primary_handle":disk_after_upsert,
        "disk_after_all_handles_drop":disk(workspace.path())?,"memory_before_vector_open":baseline_memory,
        "memory_after_upsert":memory_after_upsert,"memory_after_all_handles_drop":memory(),
        "total_executable_arm_ms":milliseconds(total_started.elapsed()),"provider_calls":0,"model_generation_ms":0,"query_embedding_ms":0,"reranking_ms":0,
        "provider_tokens":0,"provider_cost_usd":0,"infrastructure_cost_usd":Value::Null}),
    )
}

fn command_text(command: &str, arguments: &[&str]) -> Option<String> {
    Command::new(command)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
}

fn main() -> DiagnosticResult<()> {
    let protocol: Arc<Protocol> = Arc::new(contextual(
        serde_json::from_str(PROTOCOL_JSON),
        "read frozen protocol",
    )?);
    if protocol.rows != 1000
        || protocol.dimensions != 3072
        || protocol.queries != 20
        || protocol.k != 10
        || protocol.writer_updates != 20
        || protocol.mixed_readers != 16
        || protocol.tokio_workers != 2
        || protocol.seed != 0xAE7E202610210004
        || protocol.protocol_version != "aether-vector-storage-diagnostic-v1"
        || protocol.backend_order != ["sqlite", "lance"]
    {
        return Err("protocol changed outside frozen diagnostic bounds".into());
    }
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() == 2 && arguments[0] == "--arm" {
        match run_arm(&arguments[1], protocol) {
            Ok(report) => {
                println!(
                    "{}",
                    contextual(serde_json::to_string(&report), "serialize arm")?
                );
                return Ok(());
            }
            Err(error) => {
                println!(
                    "{}",
                    json!({"backend":arguments[1],"passed":false,"initialization_or_driver_error":error.to_string()})
                );
                return Err(error);
            }
        }
    }
    if !arguments.is_empty() {
        return Err("usage: vector_storage_diagnostic (no store paths accepted)".into());
    }
    let scratch = scratch_root()?;
    let mut arms = Vec::new();
    for backend in &protocol.backend_order {
        let child_started = Instant::now();
        let output = contextual(
            Command::new(contextual(
                std::env::current_exe(),
                "locate diagnostic executable",
            )?)
            .args(["--arm", backend])
            .output(),
            "launch isolated backend arm",
        )?;
        let child_process_wall_ms = milliseconds(child_started.elapsed());
        let report: Value = match serde_json::from_slice(&output.stdout) {
            Ok(report) => report,
            Err(error) => {
                json!({"passed":false,"driver_error":format!("decode arm result: {error}"),
                "raw_stdout":String::from_utf8_lossy(&output.stdout)})
            }
        };
        arms.push(json!({"process_success":output.status.success(),"child_process_wall_ms":child_process_wall_ms,
            "child_wall_scope":"parent timing of startup through exit/stdout collection, including failed initialization; missing child phase telemetry stays absent","report":report,
            "stderr":String::from_utf8_lossy(&output.stderr).chars().take(4000).collect::<String>()}));
    }
    let passed = arms.len() == 2
        && arms
            .iter()
            .all(|arm| arm["process_success"] == true && arm["report"]["passed"] == true)
        && arms[0]["report"]["dataset_hash"] == arms[1]["report"]["dataset_hash"]
        && arms[0]["report"]["query_hash"] == arms[1]["report"]["query_hash"]
        && arms[0]["report"]["oracle_hash"] == arms[1]["report"]["oracle_hash"];
    let cpu = fs::read_to_string("/proc/cpuinfo").ok().and_then(|text| {
        text.lines()
            .find(|line| line.starts_with("model name"))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_owned())
    });
    println!(
        "{}",
        contextual(
            serde_json::to_string_pretty(
                &json!({"protocol":serde_json::from_str::<Value>(PROTOCOL_JSON)?,
        "protocol_hash":content_hash(PROTOCOL_JSON),"driver_source_hash":content_hash(include_str!("vector_storage_diagnostic.rs")),
        "cargo_lock_hash":content_hash(include_str!("../../../Cargo.lock")),"source_head":command_text("git",&["rev-parse","HEAD"]),
        "source_worktree_status_hash":command_text("git",&["status","--porcelain"]).map(|text|content_hash(&text)),
        "tracked_storage_diff_hash":command_text("git",&["diff","--binary","HEAD","--","crates/aether-store","crates/aether-config","crates/aether-core"]).map(|text|content_hash(&text)),
        "rustc":command_text("rustc",&["--version"]),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "cpu_model":cpu,"mem_total":fs::read_to_string("/proc/meminfo").ok().and_then(|text|text.lines().find(|line|line.starts_with("MemTotal:")).map(str::to_owned)),
        "filesystem_descriptor":scratch.to_str().and_then(|path|command_text("df",&["-T",path])),"available_parallelism":std::thread::available_parallelism().ok().map(std::num::NonZeroUsize::get),
        "debug_assertions":cfg!(debug_assertions),"scratch_root":scratch,"compilation_included":false,
        "os_cache_flushed":false,"repetitions_per_backend":1,"passed":passed,"arms":arms,
        "interpretation":"synthetic database diagnostic; no semantic/model-quality or production-performance conclusion"})
            ),
            "serialize full diagnostic"
        )?
    );
    if !passed {
        return Err("storage diagnostic correctness/receipt gate failed; retained results are not a performance promotion".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(query: usize) -> QueryReceipt {
        QueryReceipt {
            reader: 0,
            query,
            start_offset_ms: 0.0,
            api_ms: 1.0,
            result_ids: vec!["a".to_owned(), "b".to_owned()],
            scores: vec![1.0, 0.5],
            recall_at_k: 1.0,
            ordered_exact_match: true,
            duplicate_ids: false,
            error: None,
        }
    }

    fn row(id: &str) -> VectorRecord {
        VectorRecord {
            symbol_id: id.to_owned(),
            sir_hash: format!("hash-{id}"),
            provider: PROVIDER.to_owned(),
            model: MODEL.to_owned(),
            embedding: vec![0.0, 0.5, 1.0],
            updated_at: 1,
        }
    }

    #[test]
    fn query_errors_and_missing_receipts_stay_in_denominator() {
        let mut failed = receipt(1);
        failed.error = Some("fixture API failure".to_owned());
        // An error never receives credit even if an inconsistent receipt reports recall.
        let summary = query_summary(&[receipt(0), failed], 4, 2, 10.0);
        assert_eq!(summary["exact_recall_gate_passed"], false);
        assert_eq!(summary["api_failures"], 1);
        assert_eq!(summary["missing_query_receipts"], 2);
        assert_eq!(
            summary["mean_recall_at_k_with_failures_in_denominator"],
            0.25
        );
        assert_eq!(summary["ordered_exact_match_fraction"], 0.25);
    }

    #[test]
    fn query_duplicate_ids_and_duplicate_receipts_cannot_pass() {
        let mut duplicate = receipt(1);
        duplicate.result_ids[1] = "a".to_owned();
        duplicate.recall_at_k = 0.5;
        duplicate.ordered_exact_match = false;
        // Detect the actual IDs, not just the driver's advisory duplicate flag.
        let summary = query_summary(&[receipt(0), duplicate], 2, 2, 10.0);
        assert_eq!(summary["exact_recall_gate_passed"], false);
        assert_eq!(summary["duplicate_id_queries"], 1);
        assert_eq!(
            summary["mean_recall_at_k_with_failures_in_denominator"],
            0.75
        );
        let summary = query_summary(&[receipt(0), receipt(0)], 2, 2, 10.0);
        assert_eq!(summary["exact_recall_gate_passed"], false);
        assert_eq!(summary["duplicate_query_receipts"], 1);
        assert_eq!(summary["missing_query_receipts"], 1);
        assert_eq!(
            summary["mean_recall_at_k_with_failures_in_denominator"],
            0.0
        );
    }

    #[test]
    fn query_gate_requires_complete_results_and_finite_scores() {
        let mut partial = receipt(1);
        partial.result_ids.pop();
        partial.scores.pop();
        partial.recall_at_k = 0.5;
        partial.ordered_exact_match = false;
        let summary = query_summary(&[receipt(0), partial], 2, 2, 10.0);
        assert_eq!(summary["exact_recall_gate_passed"], false);
        assert_eq!(
            summary["mean_recall_at_k_with_failures_in_denominator"],
            0.75
        );
        let mut nonfinite = receipt(0);
        nonfinite.scores[0] = f32::NAN;
        assert_eq!(
            query_summary(&[nonfinite], 1, 2, 10.0)["exact_recall_gate_passed"],
            false
        );
        assert_eq!(
            query_summary(&[receipt(0)], 1, 2, 10.0)["exact_recall_gate_passed"],
            true
        );
    }

    #[test]
    fn payload_gate_checks_initial_rows_and_changed_plus_unchanged_metadata() {
        let expected = vec![row("a"), row("b"), row("c")];
        let initial = payload_summary(&expected, 0, Ok(expected.clone()));
        assert_eq!(initial["payloads_and_metadata_gate_passed"], true);
        assert_eq!(initial["matched_rows"], 3);
        let mut mixed = expected.clone();
        mixed[0].sir_hash = "rewrite-0".to_owned();
        mixed[0].updated_at = 2;
        let summary = payload_summary(&expected, 1, Ok(mixed.clone()));
        assert_eq!(summary["payloads_and_metadata_gate_passed"], true);
        assert_eq!(summary["expected_changed_rows"], 1);
        assert_eq!(summary["expected_unchanged_rows"], 2);
        mixed[2].updated_at = 2; // An untouched row must retain its original metadata.
        let summary = payload_summary(&expected, 1, Ok(mixed));
        assert_eq!(summary["payloads_and_metadata_gate_passed"], false);
        assert_eq!(summary["mismatched_ids"], json!(["c"]));
    }

    #[test]
    fn payload_missing_duplicate_unexpected_and_error_rows_cannot_pass() {
        let expected = vec![row("a"), row("b"), row("c"), row("d")];
        let mut mismatch = row("c");
        mismatch.provider = "wrong-provider".to_owned();
        let observed = vec![row("a"), row("b"), row("b"), mismatch, row("unexpected")];
        let summary = payload_summary(&expected, 0, Ok(observed));
        assert_eq!(summary["payloads_and_metadata_gate_passed"], false);
        assert_eq!(summary["missing_ids"], json!(["d"]));
        assert_eq!(summary["duplicate_ids"], json!(["b"]));
        assert_eq!(summary["unexpected_ids"], json!(["unexpected"]));
        assert_eq!(summary["mismatched_ids"], json!(["c"]));
        assert_eq!(
            summary["exact_rows_fraction_with_failures_in_denominator"],
            0.25
        );
        let failure = payload_summary(&expected, 0, Err("fixture read failure".to_owned()));
        assert_eq!(failure["payloads_and_metadata_gate_passed"], false);
        assert_eq!(failure["missing_ids"], json!(["a", "b", "c", "d"]));
        assert_eq!(failure["api_error"], "fixture read failure");
        assert_eq!(
            failure["exact_rows_fraction_with_failures_in_denominator"],
            0.0
        );
    }

    #[test]
    fn payload_comparison_is_bitwise_and_checks_model_and_hash() {
        let expected = vec![row("a")];
        let mut changed = expected.clone();
        changed[0].embedding[0] = -0.0;
        assert_eq!(
            payload_summary(&expected, 0, Ok(changed))["payloads_and_metadata_gate_passed"],
            false
        );
        let mut changed = expected.clone();
        changed[0].model = "wrong-model".to_owned();
        assert_eq!(
            payload_summary(&expected, 0, Ok(changed))["payloads_and_metadata_gate_passed"],
            false
        );
        let mut changed = expected.clone();
        changed[0].sir_hash = "wrong-hash".to_owned();
        assert_eq!(
            payload_summary(&expected, 0, Ok(changed))["payloads_and_metadata_gate_passed"],
            false
        );
    }
}
