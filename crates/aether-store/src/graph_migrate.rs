#![cfg(feature = "legacy-cozo")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{GraphBackend, load_workspace_config, save_workspace_config};
use serde::{Deserialize, Serialize};

use super::{CozoGraphStore, GraphStore, StoreError, SurrealGraphStore};

#[derive(Debug, Clone)]
pub struct GraphMigrationOptions {
    pub workspace_root: PathBuf,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GraphMigrationResult {
    pub dry_run: bool,
    pub symbols_migrated: usize,
    pub edges_migrated: usize,
    pub backup_dir: Option<PathBuf>,
    pub snapshot_path: Option<PathBuf>,
    pub config_updated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GraphMigrationSnapshot {
    symbols: Vec<super::SymbolRecord>,
    edges: Vec<super::ResolvedEdge>,
}

pub async fn migrate_cozo_to_surreal(
    workspace_root: impl AsRef<Path>,
    dry_run: bool,
) -> Result<GraphMigrationResult, StoreError> {
    let workspace_root = workspace_root.as_ref().to_path_buf();
    refuse_if_aetherd_running(&workspace_root)?;

    let cozo = CozoGraphStore::open(&workspace_root)?;
    let symbols = cozo.list_all_symbols_for_migration()?;
    let edges = cozo.list_all_edges_for_migration()?;

    let aether_dir = workspace_root.join(".aether");
    fs::create_dir_all(&aether_dir)?;
    let timestamp = unix_timestamp_secs();
    let snapshot_path = aether_dir.join(format!("graph_migrate_export.{timestamp}.json"));

    let snapshot = GraphMigrationSnapshot {
        symbols: symbols.clone(),
        edges: edges.clone(),
    };
    fs::write(&snapshot_path, serde_json::to_vec_pretty(&snapshot)?)?;

    if dry_run {
        return Ok(GraphMigrationResult {
            dry_run: true,
            symbols_migrated: symbols.len(),
            edges_migrated: edges.len(),
            backup_dir: None,
            snapshot_path: Some(snapshot_path),
            config_updated: false,
        });
    }

    let backup_dir = backup_graph_storage(&workspace_root, timestamp)?;
    let surreal_dir = workspace_root.join(".aether").join("graph");
    if surreal_dir.exists() {
        fs::remove_dir_all(&surreal_dir)?;
    }

    let surreal = SurrealGraphStore::open(&workspace_root).await?;
    for symbol in &symbols {
        surreal.upsert_symbol_node(symbol).await?;
    }
    for edge in &edges {
        surreal.upsert_edge(edge).await?;
    }

    let symbol_count = surreal_count(surreal.db(), "symbol").await?;
    let edge_count = surreal_count(surreal.db(), "depends_on").await?;
    if symbol_count != symbols.len() || edge_count != edges.len() {
        return Err(StoreError::Graph(format!(
            "migration verification failed: expected symbols={}, edges={} got symbols={}, edges={}",
            symbols.len(),
            edges.len(),
            symbol_count,
            edge_count
        )));
    }

    let mut config = load_workspace_config(&workspace_root)?;
    config.storage.graph_backend = GraphBackend::Surreal;
    save_workspace_config(&workspace_root, &config)?;

    Ok(GraphMigrationResult {
        dry_run: false,
        symbols_migrated: symbols.len(),
        edges_migrated: edges.len(),
        backup_dir: Some(backup_dir),
        snapshot_path: Some(snapshot_path),
        config_updated: true,
    })
}

fn refuse_if_aetherd_running(workspace_root: &Path) -> Result<(), StoreError> {
    let aether_dir = workspace_root.join(".aether");
    let pid_candidates = [
        aether_dir.join("aetherd.pid"),
        aether_dir.join("aetherd.lock"),
    ];
    if let Some(path) = pid_candidates.iter().find(|path| path.exists()) {
        return Err(StoreError::Graph(format!(
            "refusing graph migration while aetherd may be running (found {})",
            path.display()
        )));
    }
    Ok(())
}

fn backup_graph_storage(workspace_root: &Path, timestamp: u64) -> Result<PathBuf, StoreError> {
    let aether_dir = workspace_root.join(".aether");
    let backup_dir = aether_dir.join(format!("graph.db.backup.{timestamp}"));
    fs::create_dir_all(&backup_dir)?;

    let cozo_file = aether_dir.join("graph.db");
    if cozo_file.exists() {
        fs::copy(&cozo_file, backup_dir.join("graph.db"))?;
    }

    let surreal_dir = aether_dir.join("graph");
    if surreal_dir.exists() {
        copy_dir_all(&surreal_dir, &backup_dir.join("graph"))?;
    }

    Ok(backup_dir)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

async fn surreal_count(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    table: &str,
) -> Result<usize, StoreError> {
    let query = format!("SELECT VALUE count() FROM {table};");
    let mut response = db
        .query(query)
        .await
        .map_err(|err| StoreError::Graph(format!("SurrealDB count query failed: {err}")))?;
    let rows: Vec<serde_json::Value> = response
        .take(0)
        .map_err(|err| StoreError::Graph(format!("SurrealDB count decode failed: {err}")))?;
    let count = rows.first().and_then(|value| value.as_u64()).unwrap_or(0);
    Ok(count as usize)
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}
