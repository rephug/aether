use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_config::load_workspace_config;
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
    pub sir_status: String,
    pub last_error: Option<String>,
    pub last_attempt_at: i64,
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
    #[error("config error: {0}")]
    Config(#[from] aether_config::ConfigError),
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
    mirror_sir_files: bool,
}

impl SqliteStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let config = load_workspace_config(workspace_root)?;
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
            mirror_sir_files: config.storage.mirror_sir_files,
        })
    }

    pub fn aether_dir(&self) -> &Path {
        &self.aether_dir
    }

    pub fn sir_dir(&self) -> &Path {
        &self.sir_dir
    }

    pub fn mirror_sir_files_enabled(&self) -> bool {
        self.mirror_sir_files
    }

    fn sir_blob_path(&self, symbol_id: &str) -> PathBuf {
        self.sir_dir.join(format!("{symbol_id}.json"))
    }

    fn upsert_sir_json_only(
        &self,
        symbol_id: &str,
        sir_json_string: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at, sir_json)
            VALUES (?1, '', 1, '', '', unixepoch(), ?2)
            ON CONFLICT(id) DO UPDATE SET
                sir_json = excluded.sir_json
            "#,
            params![symbol_id, sir_json_string],
        )?;

        Ok(())
    }

    fn read_sir_json_from_db(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT sir_json
            FROM sir
            WHERE id = ?1
            "#,
        )?;

        let json = stmt
            .query_row(params![symbol_id], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .flatten()
            .filter(|value| !value.trim().is_empty());

        Ok(json)
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
        self.conn
            .execute("DELETE FROM sir WHERE id = ?1", params![symbol_id])?;

        let path = self.sir_blob_path(symbol_id);
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

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
        self.upsert_sir_json_only(symbol_id, sir_json_string)?;

        if self.mirror_sir_files {
            let path = self.sir_blob_path(symbol_id);
            if let Err(err) = fs::write(path, sir_json_string) {
                eprintln!("aether-store: mirror write failed for symbol {symbol_id}: {err}");
            }
        }

        Ok(())
    }

    fn read_sir_blob(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        if let Some(json) = self.read_sir_json_from_db(symbol_id)? {
            return Ok(Some(json));
        }

        let path = self.sir_blob_path(symbol_id);

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(path)?;
        self.upsert_sir_json_only(symbol_id, &content)?;
        Ok(Some(content))
    }

    fn upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO sir (
                id, sir_hash, sir_version, provider, model, updated_at,
                sir_status, last_error, last_attempt_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                sir_hash = excluded.sir_hash,
                sir_version = excluded.sir_version,
                provider = excluded.provider,
                model = excluded.model,
                updated_at = excluded.updated_at,
                sir_status = excluded.sir_status,
                last_error = excluded.last_error,
                last_attempt_at = excluded.last_attempt_at
            "#,
            params![
                record.id,
                record.sir_hash,
                record.sir_version,
                record.provider,
                record.model,
                record.updated_at,
                record.sir_status,
                record.last_error,
                record.last_attempt_at,
            ],
        )?;

        Ok(())
    }

    fn get_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                id,
                sir_hash,
                sir_version,
                provider,
                model,
                updated_at,
                sir_status,
                last_error,
                last_attempt_at
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
                    sir_status: row.get(6)?,
                    last_error: row.get(7)?,
                    last_attempt_at: row.get(8)?,
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
            updated_at INTEGER NOT NULL,
            sir_json TEXT
        );
        "#,
    )?;

    if !table_has_column(conn, "sir", "sir_json")? {
        conn.execute("ALTER TABLE sir ADD COLUMN sir_json TEXT", [])?;
    }

    ensure_sir_column(conn, "sir_status", "TEXT NOT NULL DEFAULT 'fresh'")?;
    ensure_sir_column(conn, "last_error", "TEXT")?;
    ensure_sir_column(conn, "last_attempt_at", "INTEGER NOT NULL DEFAULT 0")?;

    conn.execute(
        "UPDATE sir SET sir_status = 'fresh' WHERE COALESCE(TRIM(sir_status), '') = ''",
        [],
    )?;
    conn.execute(
        "UPDATE sir SET last_attempt_at = updated_at WHERE last_attempt_at = 0",
        [],
    )?;

    Ok(())
}

fn ensure_sir_column(
    conn: &Connection,
    column_name: &str,
    column_definition: &str,
) -> Result<(), StoreError> {
    if table_has_column(conn, "sir", column_name)? {
        return Ok(());
    }

    let sql = format!("ALTER TABLE sir ADD COLUMN {column_name} {column_definition}");
    conn.execute(&sql, [])?;
    Ok(())
}

fn table_has_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, StoreError> {
    let sql = format!("PRAGMA table_info({table_name})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;

    for row in rows {
        if row?.eq_ignore_ascii_case(column_name) {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::fs;

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
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: 1_700_000_100,
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
        assert!(store.mirror_sir_files_enabled());
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
    fn read_sir_blob_prefers_sqlite_when_mirror_is_missing() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        store
            .write_sir_blob("sym-1", "{\"intent\":\"db-primary\"}")
            .expect("write blob");

        let mirror_path = workspace.join(".aether/sir/sym-1.json");
        fs::remove_file(&mirror_path).expect("remove mirror");

        let loaded = store.read_sir_blob("sym-1").expect("read from sqlite");
        assert_eq!(loaded.as_deref(), Some("{\"intent\":\"db-primary\"}"));

        drop(store);

        let reopened = SqliteStore::open(workspace).expect("reopen store");
        let reopened_loaded = reopened.read_sir_blob("sym-1").expect("read after reopen");
        assert_eq!(
            reopened_loaded.as_deref(),
            Some("{\"intent\":\"db-primary\"}")
        );
    }

    #[test]
    fn read_sir_blob_backfills_sqlite_from_mirror_without_overwriting_meta() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let meta = SirMetaRecord {
            id: "sym-legacy".to_owned(),
            sir_hash: "legacy-hash".to_owned(),
            sir_version: 3,
            provider: "legacy-provider".to_owned(),
            model: "legacy-model".to_owned(),
            updated_at: 1_700_111_222,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: 1_700_111_222,
        };
        store
            .upsert_sir_meta(meta.clone())
            .expect("upsert legacy metadata");

        let mirror_path = workspace.join(".aether/sir/sym-legacy.json");
        fs::write(&mirror_path, "{\"intent\":\"from-mirror\"}").expect("write mirror");

        let first_read = store.read_sir_blob("sym-legacy").expect("first read");
        assert_eq!(first_read.as_deref(), Some("{\"intent\":\"from-mirror\"}"));

        fs::remove_file(&mirror_path).expect("remove mirror");

        let second_read = store.read_sir_blob("sym-legacy").expect("second read");
        assert_eq!(second_read.as_deref(), Some("{\"intent\":\"from-mirror\"}"));

        let meta_after = store
            .get_sir_meta("sym-legacy")
            .expect("read metadata after backfill");
        assert_eq!(meta_after, Some(meta));
    }

    #[test]
    fn mirror_write_can_be_disabled_via_config() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "auto"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = false
"#,
        )
        .expect("write config");

        let store = SqliteStore::open(workspace).expect("open store");
        assert!(!store.mirror_sir_files_enabled());

        store
            .write_sir_blob("sym-1", "{\"intent\":\"sqlite-only\"}")
            .expect("write sqlite-only");

        let mirror_path = workspace.join(".aether/sir/sym-1.json");
        assert!(!mirror_path.exists());

        let loaded = store.read_sir_blob("sym-1").expect("read sqlite-only");
        assert_eq!(loaded.as_deref(), Some("{\"intent\":\"sqlite-only\"}"));
    }

    #[test]
    fn mark_removed_deletes_symbol_row() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_symbol(symbol_record())
            .expect("upsert symbol before delete");
        store
            .write_sir_blob("sym-1", "{\"intent\":\"to-remove\"}")
            .expect("write sir before delete");
        store.mark_removed("sym-1").expect("mark removed");

        let list = store
            .list_symbols_for_file("src/lib.rs")
            .expect("list after delete");
        assert!(list.is_empty());

        let sir = store.read_sir_blob("sym-1").expect("sir after delete");
        assert!(sir.is_none());
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

    #[test]
    fn open_store_migrates_legacy_sir_table_with_stale_defaults() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let aether_dir = workspace.join(".aether");
        let sir_dir = aether_dir.join("sir");
        fs::create_dir_all(&sir_dir).expect("create legacy aether dirs");

        let sqlite_path = aether_dir.join("meta.sqlite");
        let conn = Connection::open(&sqlite_path).expect("open legacy sqlite");
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
        )
        .expect("create legacy schema");

        conn.execute(
            "INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["legacy-sym", "legacy-hash", 1i64, "mock", "mock", 1_700_000_500i64],
        )
        .expect("insert legacy sir row");
        drop(conn);

        let store = SqliteStore::open(workspace).expect("open migrated store");
        let migrated = store
            .get_sir_meta("legacy-sym")
            .expect("load migrated row")
            .expect("row exists");

        assert_eq!(migrated.sir_status, "fresh");
        assert_eq!(migrated.last_error, None);
        assert_eq!(migrated.last_attempt_at, migrated.updated_at);
    }
}
