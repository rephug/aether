use rusqlite::{OptionalExtension, params};

use crate::{SqliteStore, StoreError, time::current_unix_timestamp_secs};

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
    current_unix_timestamp_secs()
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

    /// Insert a contract unless the symbol already has one (active or not)
    /// with exactly the same clause text. Returns the new row id, or `None`
    /// when an existing row blocked the insert.
    pub fn insert_intent_contract_if_absent(
        &self,
        symbol_id: &str,
        clause_type: &str,
        clause_text: &str,
        clause_embedding_json: Option<&str>,
        created_by: &str,
    ) -> Result<Option<i64>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            r#"
            INSERT INTO intent_contracts
                (symbol_id, clause_type, clause_text, clause_embedding_json,
                 created_at, created_by, active, violation_streak)
            SELECT ?1, ?2, ?3, ?4, ?5, ?6, 1, 0
            WHERE NOT EXISTS (
                SELECT 1 FROM intent_contracts WHERE symbol_id = ?1 AND clause_text = ?3
            )
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
        if changed == 0 {
            Ok(None)
        } else {
            Ok(Some(conn.last_insert_rowid()))
        }
    }

    /// List a symbol's contracts ordered by id, optionally including
    /// deactivated rows. Same as [`Self::list_contracts_for_symbol`], which
    /// keeps the active-only case on the partial index.
    pub fn list_intent_contracts_for_symbol(
        &self,
        symbol_id: &str,
        include_inactive: bool,
    ) -> Result<Vec<IntentContractRecord>, StoreError> {
        self.list_contracts_for_symbol(symbol_id, include_inactive)
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

    /// Lists contracts for `symbol_id`, ordered by id.
    ///
    /// With `include_inactive = false` this is identical to
    /// [`Self::list_active_contracts_for_symbol`]; with `true` it also returns
    /// deactivated (removed) contracts, distinguishable by their `active` flag.
    pub fn list_contracts_for_symbol(
        &self,
        symbol_id: &str,
        include_inactive: bool,
    ) -> Result<Vec<IntentContractRecord>, StoreError> {
        if !include_inactive {
            return self.list_active_contracts_for_symbol(symbol_id);
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, symbol_id, clause_type, clause_text, clause_embedding_json,
                   created_at, created_by, active, violation_streak
            FROM intent_contracts
            WHERE symbol_id = ?1
            ORDER BY id
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id], map_contract_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Lists contracts across all symbols, ordered by symbol id then id.
    ///
    /// With `include_inactive = false` this is identical to
    /// [`Self::list_all_active_contracts`]; with `true` it also returns
    /// deactivated (removed) contracts, distinguishable by their `active` flag.
    pub fn list_all_contracts(
        &self,
        include_inactive: bool,
    ) -> Result<Vec<IntentContractRecord>, StoreError> {
        if !include_inactive {
            return self.list_all_active_contracts();
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, symbol_id, clause_type, clause_text, clause_embedding_json,
                   created_at, created_by, active, violation_streak
            FROM intent_contracts
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

    pub fn list_violations_for_symbol_and_contract(
        &self,
        symbol_id: &str,
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
            WHERE symbol_id = ?1 AND contract_id = ?2
            ORDER BY detected_at DESC
            LIMIT ?3
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id, contract_id, limit], map_violation_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_recent_violations(
        &self,
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
            ORDER BY detected_at DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit], map_violation_row)?;
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
    fn list_violations_for_symbol_and_contract_filters_on_both() {
        let (store, _dir) = open_test_store();
        let first = store
            .insert_intent_contract("sym_abc", "must", "clause one", None, "human")
            .unwrap();
        let second = store
            .insert_intent_contract("sym_abc", "must", "clause two", None, "human")
            .unwrap();
        let first_violation = store
            .insert_intent_violation(first, "sym_abc", 1, "embedding_fail", None, None)
            .unwrap();
        store
            .insert_intent_violation(second, "sym_abc", 2, "embedding_fail", None, None)
            .unwrap();
        store
            .insert_intent_violation(first, "sym_other", 3, "embedding_fail", None, None)
            .unwrap();

        let violations = store
            .list_violations_for_symbol_and_contract("sym_abc", first, 10)
            .unwrap();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].id, first_violation);

        let none = store
            .list_violations_for_symbol_and_contract("sym_abc", 9999, 10)
            .unwrap();
        assert!(none.is_empty());
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

    fn ids_and_flags(contracts: &[IntentContractRecord]) -> Vec<(i64, bool)> {
        contracts
            .iter()
            .map(|contract| (contract.id, contract.active))
            .collect()
    }

    #[test]
    fn list_contracts_for_symbol_honors_include_inactive() {
        let (store, _dir) = open_test_store();
        let kept = store
            .insert_intent_contract("sym_a", "must", "kept clause", None, "human")
            .unwrap();
        let removed = store
            .insert_intent_contract("sym_a", "must_not", "removed clause", None, "human")
            .unwrap();
        let other = store
            .insert_intent_contract("sym_b", "must", "other symbol", None, "human")
            .unwrap();
        store.deactivate_contract(removed).unwrap();

        let active_only = store.list_contracts_for_symbol("sym_a", false).unwrap();
        assert_eq!(ids_and_flags(&active_only), vec![(kept, true)]);
        assert_eq!(
            active_only,
            store.list_active_contracts_for_symbol("sym_a").unwrap()
        );

        let with_inactive = store.list_contracts_for_symbol("sym_a", true).unwrap();
        assert_eq!(
            ids_and_flags(&with_inactive),
            vec![(kept, true), (removed, false)]
        );
        assert_eq!(with_inactive[1].clause_text, "removed clause");
        assert!(with_inactive.iter().all(|c| c.symbol_id == "sym_a"));

        let other_symbol = store.list_contracts_for_symbol("sym_b", true).unwrap();
        assert_eq!(ids_and_flags(&other_symbol), vec![(other, true)]);

        assert!(
            store
                .list_contracts_for_symbol("sym_missing", true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn list_all_contracts_honors_include_inactive() {
        let (store, _dir) = open_test_store();
        let b_kept = store
            .insert_intent_contract("sym_b", "must", "b kept", None, "human")
            .unwrap();
        let a_removed = store
            .insert_intent_contract("sym_a", "must", "a removed", None, "human")
            .unwrap();
        let a_kept = store
            .insert_intent_contract("sym_a", "preserves", "a kept", None, "agent")
            .unwrap();
        store.deactivate_contract(a_removed).unwrap();

        let active_only = store.list_all_contracts(false).unwrap();
        assert_eq!(
            ids_and_flags(&active_only),
            vec![(a_kept, true), (b_kept, true)]
        );
        assert_eq!(active_only, store.list_all_active_contracts().unwrap());

        let with_inactive = store.list_all_contracts(true).unwrap();
        assert_eq!(
            ids_and_flags(&with_inactive),
            vec![(a_removed, false), (a_kept, true), (b_kept, true)]
        );
    }

    #[test]
    fn list_recent_violations_returns_ordered_by_detected_at_desc() {
        let (store, _dir) = open_test_store();
        let cid = store
            .insert_intent_contract("sym_a", "must", "clause a", None, "human")
            .unwrap();
        store
            .insert_intent_violation(cid, "sym_a", 1, "embedding_fail", Some(0.3), None)
            .unwrap();
        store
            .insert_intent_violation(cid, "sym_a", 2, "embedding_fail", Some(0.4), None)
            .unwrap();

        let violations = store.list_recent_violations(10).unwrap();
        assert_eq!(violations.len(), 2);
        // Most recent first
        assert!(violations[0].detected_at >= violations[1].detected_at);
    }

    #[test]
    fn list_intent_contracts_for_symbol_respects_inactive_flag() {
        let (store, _dir) = open_test_store();
        let first = store
            .insert_intent_contract("sym_a", "must", "clause one", None, "human")
            .unwrap();
        let second = store
            .insert_intent_contract("sym_a", "must", "clause two", None, "inferred")
            .unwrap();
        store
            .insert_intent_contract("sym_b", "must", "other symbol", None, "human")
            .unwrap();
        store.deactivate_contract(first).unwrap();

        let active_only = store
            .list_intent_contracts_for_symbol("sym_a", false)
            .unwrap();
        assert_eq!(active_only.len(), 1);
        assert_eq!(active_only[0].id, second);

        let all = store
            .list_intent_contracts_for_symbol("sym_a", true)
            .unwrap();
        assert_eq!(
            all.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![first, second]
        );
        assert!(!all[0].active);
        assert!(all.iter().all(|c| c.symbol_id == "sym_a"));
    }

    #[test]
    fn insert_intent_contract_if_absent_dedupes_on_exact_text() {
        let (store, _dir) = open_test_store();
        let id = store
            .insert_intent_contract_if_absent(
                "sym_a",
                "must",
                "Rejects zero amounts.",
                Some("[1.0,0.0]"),
                "inferred",
            )
            .unwrap()
            .expect("first insert should create a row");

        let row = store.get_intent_contract(id).unwrap().unwrap();
        assert!(row.active);
        assert_eq!(row.violation_streak, 0);
        assert_eq!(row.created_by, "inferred");
        assert_eq!(row.clause_type, "must");
        assert_eq!(row.clause_embedding_json.as_deref(), Some("[1.0,0.0]"));

        let again = store
            .insert_intent_contract_if_absent(
                "sym_a",
                "must",
                "Rejects zero amounts.",
                None,
                "inferred",
            )
            .unwrap();
        assert_eq!(again, None);

        store.deactivate_contract(id).unwrap();
        let after_deactivate = store
            .insert_intent_contract_if_absent(
                "sym_a",
                "must",
                "Rejects zero amounts.",
                None,
                "inferred",
            )
            .unwrap();
        assert_eq!(after_deactivate, None);

        let other_symbol = store
            .insert_intent_contract_if_absent(
                "sym_b",
                "must",
                "Rejects zero amounts.",
                None,
                "inferred",
            )
            .unwrap();
        assert!(other_symbol.is_some());
        assert_eq!(
            store
                .list_intent_contracts_for_symbol("sym_a", true)
                .unwrap()
                .len(),
            1
        );
    }
}
