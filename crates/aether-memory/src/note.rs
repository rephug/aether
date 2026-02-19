use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{MemoryError, compute_content_hash, compute_note_id, current_unix_timestamp_millis};
use aether_store::{
    ProjectEntityRefRecord, ProjectNoteRecord, ProjectNoteVectorRecord, SqliteStore, Store,
    open_vector_store,
};

const EMBEDDING_CONTENT_MAX_BYTES: usize = 2 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteSourceType {
    Manual,
    Session,
    Agent,
    Import,
}

impl NoteSourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Session => "session",
            Self::Agent => "agent",
            Self::Import => "import",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectNote {
    pub note_id: String,
    pub content: String,
    pub content_hash: String,
    pub source_type: String,
    pub source_agent: Option<String>,
    pub tags: Vec<String>,
    pub entity_refs: Vec<EntityRef>,
    pub file_refs: Vec<String>,
    pub symbol_refs: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
    pub is_archived: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RememberAction {
    Created,
    UpdatedExisting,
}

impl RememberAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::UpdatedExisting => "updated_existing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberRequest {
    pub content: String,
    pub source_type: NoteSourceType,
    pub source_agent: Option<String>,
    pub tags: Vec<String>,
    pub entity_refs: Vec<EntityRef>,
    pub file_refs: Vec<String>,
    pub symbol_refs: Vec<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberResult {
    pub note: ProjectNote,
    pub action: RememberAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListNotesRequest {
    pub limit: u32,
    pub since_epoch_ms: Option<i64>,
    pub include_archived: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteEmbeddingRequest {
    pub note_id: String,
    pub provider: String,
    pub model: String,
    pub embedding: Vec<f32>,
    pub content: String,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ProjectMemoryService {
    workspace: PathBuf,
}

impl ProjectMemoryService {
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub(crate) fn open_store(&self) -> Result<SqliteStore, MemoryError> {
        Ok(SqliteStore::open(&self.workspace)?)
    }

    pub fn remember(&self, request: RememberRequest) -> Result<RememberResult, MemoryError> {
        let content = request.content.trim();
        if content.is_empty() {
            return Err(MemoryError::InvalidInput(
                "content must not be empty".to_owned(),
            ));
        }

        let now_ms = request
            .now_ms
            .unwrap_or_else(current_unix_timestamp_millis)
            .max(0);
        let content_hash = compute_content_hash(content);
        let tags = normalize_tags(request.tags);

        let store = self.open_store()?;
        if let Some(mut existing) =
            store.find_project_note_by_content_hash(content_hash.as_str(), false)?
        {
            existing.tags = merge_tags(existing.tags.as_slice(), tags.as_slice());
            existing.updated_at = now_ms;
            existing.access_count = existing.access_count.saturating_add(1);
            store.upsert_project_note(existing.clone())?;
            return Ok(RememberResult {
                note: project_note_from_store(existing),
                action: RememberAction::UpdatedExisting,
            });
        }

        let note_record = ProjectNoteRecord {
            note_id: compute_note_id(content, now_ms),
            content: content.to_owned(),
            content_hash,
            source_type: request.source_type.as_str().to_owned(),
            source_agent: normalize_optional(request.source_agent),
            tags,
            entity_refs: normalize_entity_refs(request.entity_refs),
            file_refs: normalize_string_values(request.file_refs),
            symbol_refs: normalize_string_values(request.symbol_refs),
            created_at: now_ms,
            updated_at: now_ms,
            access_count: 0,
            last_accessed_at: None,
            is_archived: false,
        };

        store.upsert_project_note(note_record.clone())?;

        Ok(RememberResult {
            note: project_note_from_store(note_record),
            action: RememberAction::Created,
        })
    }

    pub async fn upsert_note_embedding(
        &self,
        request: NoteEmbeddingRequest,
    ) -> Result<(), MemoryError> {
        let note_id = request.note_id.trim();
        if note_id.is_empty() {
            return Err(MemoryError::InvalidInput(
                "note_id must not be empty".to_owned(),
            ));
        }

        if request.embedding.is_empty() {
            return Ok(());
        }

        let vector_store = open_vector_store(&self.workspace).await?;
        vector_store
            .upsert_project_note_embedding(ProjectNoteVectorRecord {
                note_id: note_id.to_owned(),
                provider: request.provider.trim().to_owned(),
                model: request.model.trim().to_owned(),
                embedding: request.embedding,
                content: truncate_content_for_embedding(request.content.as_str()),
                created_at: request.created_at.max(0),
                updated_at: request.updated_at.unwrap_or(request.created_at).max(0),
            })
            .await?;

        Ok(())
    }

    pub fn list_notes(&self, request: ListNotesRequest) -> Result<Vec<ProjectNote>, MemoryError> {
        let store = self.open_store()?;
        let records = store.list_project_notes(
            request.limit.clamp(1, 100),
            request.since_epoch_ms,
            request.include_archived,
        )?;

        Ok(records.into_iter().map(project_note_from_store).collect())
    }

    pub fn get_note(&self, note_id: &str) -> Result<Option<ProjectNote>, MemoryError> {
        let store = self.open_store()?;
        Ok(store
            .get_project_note(note_id)?
            .map(project_note_from_store))
    }
}

pub(crate) fn project_note_from_store(record: ProjectNoteRecord) -> ProjectNote {
    ProjectNote {
        note_id: record.note_id,
        content: record.content,
        content_hash: record.content_hash,
        source_type: record.source_type,
        source_agent: record.source_agent,
        tags: record.tags,
        entity_refs: record
            .entity_refs
            .into_iter()
            .map(|item| EntityRef {
                kind: item.kind,
                id: item.id,
            })
            .collect(),
        file_refs: record.file_refs,
        symbol_refs: record.symbol_refs,
        created_at: record.created_at,
        updated_at: record.updated_at,
        access_count: record.access_count,
        last_accessed_at: record.last_accessed_at,
        is_archived: record.is_archived,
    }
}

pub(crate) fn normalize_tags(values: Vec<String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for value in values {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        set.insert(normalized);
    }
    set.into_iter().collect()
}

pub(crate) fn merge_tags(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for tag in existing {
        let normalized = tag.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        set.insert(normalized);
    }
    for tag in incoming {
        let normalized = tag.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        set.insert(normalized);
    }
    set.into_iter().collect()
}

pub(crate) fn normalize_string_values(values: Vec<String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for value in values {
        let normalized = value.trim();
        if normalized.is_empty() {
            continue;
        }
        set.insert(normalized.to_owned());
    }
    set.into_iter().collect()
}

pub(crate) fn normalize_entity_refs(values: Vec<EntityRef>) -> Vec<ProjectEntityRefRecord> {
    let mut set = BTreeSet::new();
    for item in values {
        let kind = item.kind.trim().to_ascii_lowercase();
        let id = item.id.trim().to_owned();
        if kind.is_empty() || id.is_empty() {
            continue;
        }
        set.insert((kind, id));
    }

    set.into_iter()
        .map(|(kind, id)| ProjectEntityRefRecord { kind, id })
        .collect()
}

pub fn truncate_content_for_embedding(content: &str) -> String {
    if content.len() <= EMBEDDING_CONTENT_MAX_BYTES {
        return content.to_owned();
    }

    let mut end = EMBEDDING_CONTENT_MAX_BYTES;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    content[..end].to_owned()
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        NoteSourceType, ProjectMemoryService, RememberAction, RememberRequest,
        truncate_content_for_embedding,
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_workspace(name: &str) -> PathBuf {
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("aether-memory-{name}-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test workspace");
        path
    }

    #[test]
    fn remember_creates_and_lists_note() {
        let workspace = test_workspace("remember-create");
        let service = ProjectMemoryService::new(&workspace);

        let result = service
            .remember(RememberRequest {
                content: "Decision log entry".to_owned(),
                source_type: NoteSourceType::Manual,
                source_agent: None,
                tags: vec!["Architecture".to_owned()],
                entity_refs: Vec::new(),
                file_refs: vec!["src/lib.rs".to_owned()],
                symbol_refs: Vec::new(),
                now_ms: Some(1_700_000_001_000),
            })
            .expect("remember note");

        assert_eq!(result.action, RememberAction::Created);
        assert_eq!(result.note.tags, vec!["architecture".to_owned()]);
        assert_eq!(result.note.file_refs, vec!["src/lib.rs".to_owned()]);

        let listed = service
            .list_notes(super::ListNotesRequest {
                limit: 10,
                since_epoch_ms: None,
                include_archived: false,
            })
            .expect("list notes");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].note_id, result.note.note_id);

        let loaded = service
            .get_note(result.note.note_id.as_str())
            .expect("get note")
            .expect("note exists");
        assert_eq!(loaded.content, "Decision log entry");
    }

    #[test]
    fn remember_dedup_updates_existing_and_merges_tags() {
        let workspace = test_workspace("remember-dedup");
        let service = ProjectMemoryService::new(&workspace);

        let first = service
            .remember(RememberRequest {
                content: "Why we selected sqlite".to_owned(),
                source_type: NoteSourceType::Agent,
                source_agent: Some("codex".to_owned()),
                tags: vec!["architecture".to_owned()],
                entity_refs: Vec::new(),
                file_refs: Vec::new(),
                symbol_refs: Vec::new(),
                now_ms: Some(1_700_000_002_000),
            })
            .expect("first remember");
        let second = service
            .remember(RememberRequest {
                content: "Why we selected sqlite".to_owned(),
                source_type: NoteSourceType::Agent,
                source_agent: Some("codex".to_owned()),
                tags: vec!["database".to_owned(), "architecture".to_owned()],
                entity_refs: Vec::new(),
                file_refs: Vec::new(),
                symbol_refs: Vec::new(),
                now_ms: Some(1_700_000_003_000),
            })
            .expect("second remember");

        assert_eq!(first.note.note_id, second.note.note_id);
        assert_eq!(second.action, RememberAction::UpdatedExisting);
        assert_eq!(
            second.note.tags,
            vec!["architecture".to_owned(), "database".to_owned()]
        );
        assert_eq!(second.note.access_count, 1);
        assert_eq!(second.note.updated_at, 1_700_000_003_000);
    }

    #[test]
    fn truncates_embedding_content_to_two_kibibytes() {
        let content = "x".repeat(3_000);
        let truncated = truncate_content_for_embedding(content.as_str());
        assert_eq!(truncated.len(), 2 * 1024);
    }
}
