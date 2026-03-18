use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, params};

use crate::{SqliteStore, StoreError};

#[derive(Debug, Clone, PartialEq)]
pub struct IntentContractRecord {
    pub id: i64,
    pub symbol_id: String,
    pub clause_type: String,
    pub clause_text: String,
    pub clause_embedding_json: Option<String>,
    pub created_at: i64,
    pub created_by: String,
    pub active: bool,
    pub violation_streak: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IntentViolationRecord {
    pub id: i64,
    pub contract_id: i64,
    pub symbol_id: String,
    pub sir_version: i64,
    pub violation_type: String,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
    pub detected_at: i64,
    pub dismissed: bool,
    pub dismissed_at: Option<i64>,
    pub dismissed_reason: Option<String>,
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

impl SqliteStore {
    pub fn insert_intent_contract(
        &self,
        symbol_id: &str,
        clause_type: &str,
        clause_text: &str,
        clause_embedding_json: Option<&str>,
        created_by: &str,
    ) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO intent_contracts
                (symbol_id, clause_type, clause_text, clause_embedding_json,
                 created_at, created_by, active, violation_streak)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 0)
            "#,
            params![
                symbol_id,
                clause_type,
                clause_text,
                clause_embedding_json,
                unix_now(),
                created_by,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_active_contracts_for_symbol(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<IntentContractRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, symbol_id, clause_type, clause_text, clause_embedding_json,
                   created_at, created_by, active, violation_streak
            FROM intent_contracts
            WHERE symbol_id = ?1 AND active = 1
            ORDER BY id
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id], map_contract_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_all_active_contracts(&self) -> Result<Vec<IntentContractRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, symbol_id, clause_type, clause_text, clause_embedding_json,
                   created_at, created_by, active, violation_streak
            FROM intent_contracts
            WHERE active = 1
            ORDER BY symbol_id, id
            "#,
        )?;
        let rows = stmt.query_map([], map_contract_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn deactivate_contract(&self, contract_id: i64) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE intent_contracts SET active = 0 WHERE id = ?1",
            params![contract_id],
        )?;
        Ok(())
    }

    pub fn update_contract_streak(&self, contract_id: i64, streak: i64) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE intent_contracts SET violation_streak = ?1 WHERE id = ?2",
            params![streak, contract_id],
        )?;
        Ok(())
    }

    pub fn reset_contract_streak(&self, contract_id: i64) -> Result<(), StoreError> {
        self.update_contract_streak(contract_id, 0)
    }

    pub fn insert_intent_violation(
        &self,
        contract_id: i64,
        symbol_id: &str,
        sir_version: i64,
        violation_type: &str,
        confidence: Option<f64>,
        reason: Option<&str>,
    ) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO intent_violations
                (contract_id, symbol_id, sir_version, violation_type,
                 confidence, reason, detected_at, dismissed)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
            "#,
            params![
                contract_id,
                symbol_id,
                sir_version,
                violation_type,
                confidence,
                reason,
                unix_now(),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_violations_for_contract(
        &self,
        contract_id: i64,
        limit: usize,
    ) -> Result<Vec<IntentViolationRecord>, StoreError> {
        let limit = limit.clamp(1, 1000);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, contract_id, symbol_id, sir_version, violation_type,
                   confidence, reason, detected_at, dismissed, dismissed_at,
                   dismissed_reason
            FROM intent_violations
            WHERE contract_id = ?1
            ORDER BY detected_at DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![contract_id, limit], map_violation_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_violations_for_symbol(
        &self,
        symbol_id: &str,
        limit: usize,
    ) -> Result<Vec<IntentViolationRecord>, StoreError> {
        let limit = limit.clamp(1, 1000);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, contract_id, symbol_id, sir_version, violation_type,
                   confidence, reason, detected_at, dismissed, dismissed_at,
                   dismissed_reason
            FROM intent_violations
            WHERE symbol_id = ?1
            ORDER BY detected_at DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id, limit], map_violation_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn dismiss_violation(&self, violation_id: i64, reason: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            UPDATE intent_violations
            SET dismissed = 1, dismissed_at = ?1, dismissed_reason = ?2
            WHERE id = ?3
            "#,
            params![unix_now(), reason, violation_id],
        )?;
        Ok(())
    }

    pub fn count_dismissed_recurrences(&self, contract_id: i64) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM intent_violations WHERE contract_id = ?1 AND dismissed = 1",
            params![contract_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    pub fn get_intent_contract(
        &self,
        contract_id: i64,
    ) -> Result<Option<IntentContractRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT id, symbol_id, clause_type, clause_text, clause_embedding_json,
                   created_at, created_by, active, violation_streak
            FROM intent_contracts
            WHERE id = ?1
            "#,
            params![contract_id],
            map_contract_row,
        )
        .optional()
        .map_err(Into::into)
    }
}

fn map_contract_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IntentContractRecord> {
    Ok(IntentContractRecord {
        id: row.get(0)?,
        symbol_id: row.get(1)?,
        clause_type: row.get(2)?,
        clause_text: row.get(3)?,
        clause_embedding_json: row.get(4)?,
        created_at: row.get(5)?,
        created_by: row.get(6)?,
        active: row.get::<_, i64>(7)? != 0,
        violation_streak: row.get(8)?,
    })
}

fn map_violation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IntentViolationRecord> {
    Ok(IntentViolationRecord {
        id: row.get(0)?,
        contract_id: row.get(1)?,
        symbol_id: row.get(2)?,
        sir_version: row.get(3)?,
        violation_type: row.get(4)?,
        confidence: row.get(5)?,
        reason: row.get(6)?,
        detected_at: row.get(7)?,
        dismissed: row.get::<_, i64>(8)? != 0,
        dismissed_at: row.get(9)?,
        dismissed_reason: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_store() -> (SqliteStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        (store, dir)
    }

    #[test]
    fn contract_insert_and_list_round_trip() {
        let (store, _dir) = open_test_store();
        let id = store
            .insert_intent_contract("sym_abc", "must", "reject zero amounts", None, "human")
            .unwrap();
        assert!(id > 0);

        let contracts = store.list_active_contracts_for_symbol("sym_abc").unwrap();
        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].clause_type, "must");
        assert_eq!(contracts[0].clause_text, "reject zero amounts");
        assert_eq!(contracts[0].created_by, "human");
        assert!(contracts[0].active);
        assert_eq!(contracts[0].violation_streak, 0);
    }

    #[test]
    fn contract_with_embedding_round_trip() {
        let (store, _dir) = open_test_store();
        let embedding = serde_json::to_string(&vec![0.1_f32, 0.2, 0.3]).unwrap();
        let id = store
            .insert_intent_contract(
                "sym_abc",
                "must_not",
                "panic on invalid input",
                Some(&embedding),
                "agent",
            )
            .unwrap();

        let contract = store.get_intent_contract(id).unwrap().unwrap();
        assert_eq!(
            contract.clause_embedding_json.as_deref(),
            Some(embedding.as_str())
        );
    }

    #[test]
    fn contract_deactivate_hides_from_active_list() {
        let (store, _dir) = open_test_store();
        let id = store
            .insert_intent_contract("sym_abc", "must", "clause 1", None, "human")
            .unwrap();
        store.deactivate_contract(id).unwrap();

        let contracts = store.list_active_contracts_for_symbol("sym_abc").unwrap();
        assert!(contracts.is_empty());

        let all = store.list_all_active_contracts().unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn violation_insert_and_list_round_trip() {
        let (store, _dir) = open_test_store();
        let contract_id = store
            .insert_intent_contract("sym_abc", "must", "clause", None, "human")
            .unwrap();
        let viol_id = store
            .insert_intent_violation(
                contract_id,
                "sym_abc",
                5,
                "embedding_fail",
                Some(0.35),
                Some("low similarity"),
            )
            .unwrap();
        assert!(viol_id > 0);

        let violations = store.list_violations_for_contract(contract_id, 10).unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].sir_version, 5);
        assert_eq!(violations[0].violation_type, "embedding_fail");
        assert!(!violations[0].dismissed);

        let by_symbol = store.list_violations_for_symbol("sym_abc", 10).unwrap();
        assert_eq!(by_symbol.len(), 1);
    }

    #[test]
    fn dismiss_violation_sets_fields() {
        let (store, _dir) = open_test_store();
        let cid = store
            .insert_intent_contract("sym_abc", "must", "clause", None, "human")
            .unwrap();
        let vid = store
            .insert_intent_violation(cid, "sym_abc", 1, "embedding_fail", None, None)
            .unwrap();

        store.dismiss_violation(vid, "false positive").unwrap();

        let violations = store.list_violations_for_contract(cid, 10).unwrap();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].dismissed);
        assert!(violations[0].dismissed_at.is_some());
        assert_eq!(
            violations[0].dismissed_reason.as_deref(),
            Some("false positive")
        );
    }

    #[test]
    fn streak_update_and_reset() {
        let (store, _dir) = open_test_store();
        let id = store
            .insert_intent_contract("sym_abc", "must", "clause", None, "human")
            .unwrap();

        store.update_contract_streak(id, 3).unwrap();
        let contract = store.get_intent_contract(id).unwrap().unwrap();
        assert_eq!(contract.violation_streak, 3);

        store.reset_contract_streak(id).unwrap();
        let contract = store.get_intent_contract(id).unwrap().unwrap();
        assert_eq!(contract.violation_streak, 0);
    }

    #[test]
    fn count_dismissed_recurrences_counts_correctly() {
        let (store, _dir) = open_test_store();
        let cid = store
            .insert_intent_contract("sym_abc", "must", "clause", None, "human")
            .unwrap();

        let v1 = store
            .insert_intent_violation(cid, "sym_abc", 1, "embedding_fail", None, None)
            .unwrap();
        let v2 = store
            .insert_intent_violation(cid, "sym_abc", 2, "embedding_fail", None, None)
            .unwrap();

        assert_eq!(store.count_dismissed_recurrences(cid).unwrap(), 0);

        store.dismiss_violation(v1, "test").unwrap();
        assert_eq!(store.count_dismissed_recurrences(cid).unwrap(), 1);

        store.dismiss_violation(v2, "test").unwrap();
        assert_eq!(store.count_dismissed_recurrences(cid).unwrap(), 2);
    }

    #[test]
    fn list_all_active_contracts_across_symbols() {
        let (store, _dir) = open_test_store();
        store
            .insert_intent_contract("sym_a", "must", "clause a", None, "human")
            .unwrap();
        store
            .insert_intent_contract("sym_b", "must_not", "clause b", None, "agent")
            .unwrap();

        let all = store.list_all_active_contracts().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].symbol_id, "sym_a");
        assert_eq!(all[1].symbol_id, "sym_b");
    }
}
