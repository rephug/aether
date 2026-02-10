use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRecord {
    pub id: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub qualified_name: String,
    pub signature_fingerprint: String,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirMetaRecord {
    pub id: String,
    pub sir_hash: String,
    pub sir_version: i64,
    pub provider: String,
    pub model: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSearchResult {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub trait Store {
    fn upsert_symbol(&self, record: SymbolRecord) -> Result<(), StoreError>;
    fn mark_removed(&self, symbol_id: &str) -> Result<(), StoreError>;
    fn list_symbols_for_file(&self, file_path: &str) -> Result<Vec<SymbolRecord>, StoreError>;
    fn search_symbols(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SymbolSearchResult>, StoreError>;

    fn write_sir_blob(&self, symbol_id: &str, sir_json_string: &str) -> Result<(), StoreError>;
    fn read_sir_blob(&self, symbol_id: &str) -> Result<Option<String>, StoreError>;

    fn upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError>;
    fn get_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, StoreError>;
}

pub struct SqliteStore {
    conn: Connection,
    aether_dir: PathBuf,
    sir_dir: PathBuf,
}

impl SqliteStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let aether_dir = workspace_root.join(".aether");
        let sir_dir = aether_dir.join("sir");
        let sqlite_path = aether_dir.join("meta.sqlite");

        fs::create_dir_all(&sir_dir)?;

        let conn = Connection::open(sqlite_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        run_migrations(&conn)?;

        Ok(Self {
            conn,
            aether_dir,
            sir_dir,
        })
    }

    pub fn aether_dir(&self) -> &Path {
        &self.aether_dir
    }

    pub fn sir_dir(&self) -> &Path {
        &self.sir_dir
    }

    fn sir_blob_path(&self, symbol_id: &str) -> PathBuf {
        self.sir_dir.join(format!("{symbol_id}.json"))
    }
}

impl Store for SqliteStore {
    fn upsert_symbol(&self, record: SymbolRecord) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO symbols (
                id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                file_path = excluded.file_path,
                language = excluded.language,
                kind = excluded.kind,
                qualified_name = excluded.qualified_name,
                signature_fingerprint = excluded.signature_fingerprint,
                last_seen_at = excluded.last_seen_at
            "#,
            params![
                record.id,
                record.file_path,
                record.language,
                record.kind,
                record.qualified_name,
                record.signature_fingerprint,
                record.last_seen_at,
            ],
        )?;

        Ok(())
    }

    fn mark_removed(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM symbols WHERE id = ?1", params![symbol_id])?;
        Ok(())
    }

    fn list_symbols_for_file(&self, file_path: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE file_path = ?1
            ORDER BY id
            "#,
        )?;

        let rows = stmt.query_map(params![file_path], |row| {
            Ok(SymbolRecord {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                qualified_name: row.get(4)?,
                signature_fingerprint: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn search_symbols(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SymbolSearchResult>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let capped_limit = limit.clamp(1, 100) as i64;
        let pattern = format!("%{query}%");

        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, qualified_name, file_path, language, kind
            FROM symbols
            WHERE LOWER(id) LIKE LOWER(?1)
               OR LOWER(qualified_name) LIKE LOWER(?1)
               OR LOWER(file_path) LIKE LOWER(?1)
               OR LOWER(language) LIKE LOWER(?1)
               OR LOWER(kind) LIKE LOWER(?1)
            ORDER BY qualified_name ASC, id ASC
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![pattern, capped_limit], |row| {
            Ok(SymbolSearchResult {
                symbol_id: row.get(0)?,
                qualified_name: row.get(1)?,
                file_path: row.get(2)?,
                language: row.get(3)?,
                kind: row.get(4)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn write_sir_blob(&self, symbol_id: &str, sir_json_string: &str) -> Result<(), StoreError> {
        let path = self.sir_blob_path(symbol_id);
        fs::write(path, sir_json_string)?;
        Ok(())
    }

    fn read_sir_blob(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        let path = self.sir_blob_path(symbol_id);

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(path)?;
        Ok(Some(content))
    }

    fn upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                sir_hash = excluded.sir_hash,
                sir_version = excluded.sir_version,
                provider = excluded.provider,
                model = excluded.model,
                updated_at = excluded.updated_at
            "#,
            params![
                record.id,
                record.sir_hash,
                record.sir_version,
                record.provider,
                record.model,
                record.updated_at,
            ],
        )?;

        Ok(())
    }

    fn get_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, sir_hash, sir_version, provider, model, updated_at
            FROM sir
            WHERE id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SirMetaRecord {
                    id: row.get(0)?,
                    sir_hash: row.get(1)?,
                    sir_version: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .optional()?;

        Ok(record)
    }
}

fn run_migrations(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS symbols (
            id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            language TEXT NOT NULL,
            kind TEXT NOT NULL,
            qualified_name TEXT NOT NULL,
            signature_fingerprint TEXT NOT NULL,
            last_seen_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sir (
            id TEXT PRIMARY KEY,
            sir_hash TEXT NOT NULL,
            sir_version INTEGER NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "#,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn symbol_record() -> SymbolRecord {
        SymbolRecord {
            id: "sym-1".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "demo::run".to_owned(),
            signature_fingerprint: "sig-a".to_owned(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn sir_meta_record() -> SirMetaRecord {
        SirMetaRecord {
            id: "sym-1".to_owned(),
            sir_hash: "hash-a".to_owned(),
            sir_version: 1,
            provider: "none".to_owned(),
            model: "none".to_owned(),
            updated_at: 1_700_000_100,
        }
    }

    fn symbol_record_ts() -> SymbolRecord {
        SymbolRecord {
            id: "sym-2".to_owned(),
            file_path: "src/app.ts".to_owned(),
            language: "typescript".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "web::render".to_owned(),
            signature_fingerprint: "sig-c".to_owned(),
            last_seen_at: 1_700_000_000,
        }
    }

    #[test]
    fn store_creates_layout_and_persists_data_without_duplicates() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let store = SqliteStore::open(workspace).expect("open store");
        assert!(store.aether_dir().exists());
        assert!(store.sir_dir().exists());
        assert!(store.aether_dir().join("meta.sqlite").exists());

        let mut record = symbol_record();
        store
            .upsert_symbol(record.clone())
            .expect("upsert symbol first time");

        record.last_seen_at = 1_700_000_200;
        record.signature_fingerprint = "sig-b".to_owned();
        store
            .upsert_symbol(record.clone())
            .expect("upsert symbol second time");

        let list = store
            .list_symbols_for_file("src/lib.rs")
            .expect("list symbols after upsert");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], record);

        store
            .write_sir_blob("sym-1", "{\"intent\":\"demo\"}")
            .expect("write blob");
        let blob = store.read_sir_blob("sym-1").expect("read blob");
        assert_eq!(blob.as_deref(), Some("{\"intent\":\"demo\"}"));

        let sir_meta = sir_meta_record();
        store
            .upsert_sir_meta(sir_meta.clone())
            .expect("upsert sir meta");
        let loaded_meta = store.get_sir_meta("sym-1").expect("get sir meta");
        assert_eq!(loaded_meta, Some(sir_meta));

        drop(store);

        let reopened = SqliteStore::open(workspace).expect("reopen store");
        let reopened_list = reopened
            .list_symbols_for_file("src/lib.rs")
            .expect("list symbols after reopen");
        assert_eq!(reopened_list.len(), 1);
        assert_eq!(reopened_list[0], record);

        let reopened_blob = reopened
            .read_sir_blob("sym-1")
            .expect("read blob after reopen");
        assert_eq!(reopened_blob.as_deref(), Some("{\"intent\":\"demo\"}"));

        let reopened_meta = reopened.get_sir_meta("sym-1").expect("meta after reopen");
        assert_eq!(reopened_meta, Some(sir_meta_record()));
    }

    #[test]
    fn mark_removed_deletes_symbol_row() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_symbol(symbol_record())
            .expect("upsert symbol before delete");
        store.mark_removed("sym-1").expect("mark removed");

        let list = store
            .list_symbols_for_file("src/lib.rs")
            .expect("list after delete");
        assert!(list.is_empty());
    }

    #[test]
    fn search_symbols_matches_by_name_path_language_and_kind() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_symbol(symbol_record())
            .expect("upsert rust symbol");
        store
            .upsert_symbol(symbol_record_ts())
            .expect("upsert ts symbol");

        let by_name = store
            .search_symbols("demo::run", 20)
            .expect("search by name");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].symbol_id, "sym-1");

        let by_path = store
            .search_symbols("src/app.ts", 20)
            .expect("search by path");
        assert_eq!(by_path.len(), 1);
        assert_eq!(by_path[0].symbol_id, "sym-2");

        let by_language = store
            .search_symbols("RUST", 20)
            .expect("search by language");
        assert_eq!(by_language.len(), 1);
        assert_eq!(by_language[0].symbol_id, "sym-1");

        let by_kind = store
            .search_symbols("function", 20)
            .expect("search by kind");
        assert_eq!(by_kind.len(), 2);
    }

    #[test]
    fn search_symbols_respects_empty_query_and_limit() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        let mut first = symbol_record();
        first.qualified_name = "alpha::run".to_owned();
        first.id = "sym-a".to_owned();
        store.upsert_symbol(first).expect("upsert first symbol");

        let mut second = symbol_record();
        second.qualified_name = "beta::run".to_owned();
        second.id = "sym-b".to_owned();
        store.upsert_symbol(second).expect("upsert second symbol");

        let empty = store.search_symbols("   ", 20).expect("search empty");
        assert!(empty.is_empty());

        let limited = store.search_symbols("::run", 1).expect("search with limit");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].qualified_name, "alpha::run");
    }
}
