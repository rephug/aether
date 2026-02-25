use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::DynError;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StalenessInfo {
    pub stale: bool,
    pub last_indexed_at: Option<i64>,
    pub staleness_minutes: Option<u64>,
    pub warning: Option<String>,
}

pub fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn compute_staleness(
    now_unix: i64,
    last_indexed_at: Option<i64>,
    warn_after_minutes: u64,
) -> StalenessInfo {
    let Some(last_indexed_at) = last_indexed_at else {
        return StalenessInfo {
            stale: false,
            last_indexed_at: None,
            staleness_minutes: None,
            warning: None,
        };
    };

    let delta_seconds = now_unix.saturating_sub(last_indexed_at).max(0) as u64;
    let staleness_minutes = delta_seconds / 60;
    let stale = staleness_minutes >= warn_after_minutes;
    let warning = stale.then(|| {
        format!(
            "Index has not been updated in {staleness_minutes} minutes. Results may be outdated."
        )
    });

    StalenessInfo {
        stale,
        last_indexed_at: Some(last_indexed_at),
        staleness_minutes: Some(staleness_minutes),
        warning,
    }
}

pub fn read_last_indexed_at(workspace_root: &Path) -> Result<Option<i64>, DynError> {
    let sqlite_path = workspace_root.join(".aether").join("meta.sqlite");
    let conn = Connection::open_with_flags(sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;

    let latest_sir = conn.query_row("SELECT MAX(updated_at) FROM sir", [], |row| row.get(0));
    match latest_sir {
        Ok(Some(ts)) => return Ok(Some(ts)),
        Ok(None) => {}
        Err(rusqlite::Error::SqliteFailure(_, _)) => {}
        Err(err) => return Err(Box::new(err)),
    }

    let latest_symbol = conn.query_row("SELECT MAX(last_seen_at) FROM symbols", [], |row| {
        row.get(0)
    });
    match latest_symbol {
        Ok(ts) => Ok(ts),
        Err(rusqlite::Error::SqliteFailure(_, _)) => Ok(None),
        Err(err) => Err(Box::new(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::compute_staleness;

    #[test]
    fn staleness_fresh_index_is_not_stale() {
        let info = compute_staleness(1_000, Some(950), 30);
        assert!(!info.stale);
        assert_eq!(info.last_indexed_at, Some(950));
        assert_eq!(info.staleness_minutes, Some(0));
        assert!(info.warning.is_none());
    }

    #[test]
    fn staleness_old_index_is_stale() {
        let info = compute_staleness(10_000, Some(7_000), 30);
        assert!(info.stale);
        assert_eq!(info.staleness_minutes, Some(50));
        assert!(info.warning.as_deref().unwrap_or_default().contains("50"));
    }

    #[test]
    fn staleness_handles_missing_timestamp() {
        let info = compute_staleness(10_000, None, 30);
        assert!(!info.stale);
        assert!(info.last_indexed_at.is_none());
        assert!(info.staleness_minutes.is_none());
        assert!(info.warning.is_none());
    }
}
