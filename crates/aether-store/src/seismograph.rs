use rusqlite::{OptionalExtension, params};

use crate::{SqliteStore, StoreError};

#[derive(Debug, Clone, PartialEq)]
pub struct SeismographMetricRecord {
    pub batch_timestamp: i64,
    pub codebase_shift: f64,
    pub semantic_velocity: f64,
    pub symbols_regenerated: i64,
    pub symbols_above_noise: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommunityStabilityRecord {
    pub community_id: String,
    pub computed_at: i64,
    pub stability: f64,
    pub symbol_count: i64,
    pub breach_count: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CascadeRecord {
    pub epicenter_symbol_id: String,
    pub chain_json: String,
    pub total_hops: i64,
    pub max_delta_sem: f64,
    pub detected_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AftershockModelRecord {
    pub trained_at: i64,
    pub training_samples: i64,
    pub weights_json: String,
    pub auc_roc: Option<f64>,
}

impl SqliteStore {
    pub fn insert_seismograph_metric(
        &self,
        record: &SeismographMetricRecord,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO metrics_seismograph
                (batch_timestamp, codebase_shift, semantic_velocity, symbols_regenerated, symbols_above_noise)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                record.batch_timestamp,
                record.codebase_shift,
                record.semantic_velocity,
                record.symbols_regenerated,
                record.symbols_above_noise,
            ],
        )?;
        Ok(())
    }

    pub fn list_seismograph_metrics(
        &self,
        limit: usize,
    ) -> Result<Vec<SeismographMetricRecord>, StoreError> {
        let limit = limit.clamp(1, 1000);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT batch_timestamp, codebase_shift, semantic_velocity,
                   symbols_regenerated, symbols_above_noise
            FROM metrics_seismograph
            ORDER BY batch_timestamp DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(SeismographMetricRecord {
                batch_timestamp: row.get(0)?,
                codebase_shift: row.get(1)?,
                semantic_velocity: row.get(2)?,
                symbols_regenerated: row.get(3)?,
                symbols_above_noise: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn latest_seismograph_metric(&self) -> Result<Option<SeismographMetricRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT batch_timestamp, codebase_shift, semantic_velocity,
                   symbols_regenerated, symbols_above_noise
            FROM metrics_seismograph
            ORDER BY batch_timestamp DESC
            LIMIT 1
            "#,
            [],
            |row| {
                Ok(SeismographMetricRecord {
                    batch_timestamp: row.get(0)?,
                    codebase_shift: row.get(1)?,
                    semantic_velocity: row.get(2)?,
                    symbols_regenerated: row.get(3)?,
                    symbols_above_noise: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn insert_community_stability(
        &self,
        record: &CommunityStabilityRecord,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO metrics_community_stability
                (community_id, computed_at, stability, symbol_count, breach_count)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                record.community_id,
                record.computed_at,
                record.stability,
                record.symbol_count,
                record.breach_count,
            ],
        )?;
        Ok(())
    }

    pub fn list_community_stability(
        &self,
        computed_at: i64,
    ) -> Result<Vec<CommunityStabilityRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT community_id, computed_at, stability, symbol_count, breach_count
            FROM metrics_community_stability
            WHERE computed_at = ?1
            ORDER BY stability ASC
            "#,
        )?;
        let rows = stmt.query_map(params![computed_at], |row| {
            Ok(CommunityStabilityRecord {
                community_id: row.get(0)?,
                computed_at: row.get(1)?,
                stability: row.get(2)?,
                symbol_count: row.get(3)?,
                breach_count: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn latest_community_stability(&self) -> Result<Vec<CommunityStabilityRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let max_ts: Option<i64> = conn
            .query_row(
                "SELECT MAX(computed_at) FROM metrics_community_stability",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        let Some(ts) = max_ts else {
            return Ok(Vec::new());
        };

        drop(conn);
        self.list_community_stability(ts)
    }

    pub fn insert_cascade(&self, record: &CascadeRecord) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO metrics_cascade
                (epicenter_symbol_id, chain_json, total_hops, max_delta_sem, detected_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                record.epicenter_symbol_id,
                record.chain_json,
                record.total_hops,
                record.max_delta_sem,
                record.detected_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_cascades(&self, limit: usize) -> Result<Vec<CascadeRecord>, StoreError> {
        let limit = limit.clamp(1, 500);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT epicenter_symbol_id, chain_json, total_hops, max_delta_sem, detected_at
            FROM metrics_cascade
            ORDER BY detected_at DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(CascadeRecord {
                epicenter_symbol_id: row.get(0)?,
                chain_json: row.get(1)?,
                total_hops: row.get(2)?,
                max_delta_sem: row.get(3)?,
                detected_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn insert_aftershock_model(
        &self,
        record: &AftershockModelRecord,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO metrics_aftershock_model
                (trained_at, training_samples, weights_json, auc_roc)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                record.trained_at,
                record.training_samples,
                record.weights_json,
                record.auc_roc,
            ],
        )?;
        Ok(())
    }

    pub fn latest_aftershock_model(&self) -> Result<Option<AftershockModelRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT trained_at, training_samples, weights_json, auc_roc
            FROM metrics_aftershock_model
            ORDER BY trained_at DESC
            LIMIT 1
            "#,
            [],
            |row| {
                Ok(AftershockModelRecord {
                    trained_at: row.get(0)?,
                    training_samples: row.get(1)?,
                    weights_json: row.get(2)?,
                    auc_roc: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteStore;

    fn open_test_store() -> (SqliteStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(dir.path()).unwrap();
        (store, dir)
    }

    #[test]
    fn seismograph_metrics_round_trip() {
        let (store, _dir) = open_test_store();
        let record = SeismographMetricRecord {
            batch_timestamp: 1000,
            codebase_shift: 0.42,
            semantic_velocity: 0.35,
            symbols_regenerated: 100,
            symbols_above_noise: 30,
        };
        store.insert_seismograph_metric(&record).unwrap();

        let latest = store.latest_seismograph_metric().unwrap().unwrap();
        assert_eq!(latest.batch_timestamp, 1000);
        assert!((latest.codebase_shift - 0.42).abs() < 1e-10);
        assert!((latest.semantic_velocity - 0.35).abs() < 1e-10);
        assert_eq!(latest.symbols_regenerated, 100);
        assert_eq!(latest.symbols_above_noise, 30);

        let all = store.list_seismograph_metrics(10).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn community_stability_round_trip() {
        let (store, _dir) = open_test_store();
        let r1 = CommunityStabilityRecord {
            community_id: "c1".to_owned(),
            computed_at: 2000,
            stability: 0.95,
            symbol_count: 50,
            breach_count: 2,
        };
        let r2 = CommunityStabilityRecord {
            community_id: "c2".to_owned(),
            computed_at: 2000,
            stability: 0.60,
            symbol_count: 30,
            breach_count: 10,
        };
        store.insert_community_stability(&r1).unwrap();
        store.insert_community_stability(&r2).unwrap();

        let by_time = store.list_community_stability(2000).unwrap();
        assert_eq!(by_time.len(), 2);
        // Ordered by stability ASC, so c2 (0.60) comes first
        assert_eq!(by_time[0].community_id, "c2");
        assert_eq!(by_time[1].community_id, "c1");

        let latest = store.latest_community_stability().unwrap();
        assert_eq!(latest.len(), 2);
    }

    #[test]
    fn cascade_round_trip() {
        let (store, _dir) = open_test_store();
        let record = CascadeRecord {
            epicenter_symbol_id: "sym_root".to_owned(),
            chain_json: r#"[{"id":"sym_root","delta":0.8}]"#.to_owned(),
            total_hops: 3,
            max_delta_sem: 0.8,
            detected_at: 3000,
        };
        store.insert_cascade(&record).unwrap();

        let cascades = store.list_cascades(10).unwrap();
        assert_eq!(cascades.len(), 1);
        assert_eq!(cascades[0].epicenter_symbol_id, "sym_root");
        assert_eq!(cascades[0].total_hops, 3);
    }

    #[test]
    fn aftershock_model_round_trip() {
        let (store, _dir) = open_test_store();
        let record = AftershockModelRecord {
            trained_at: 4000,
            training_samples: 500,
            weights_json: r#"[0.1, 0.2, 0.3, 0.4, 0.5]"#.to_owned(),
            auc_roc: Some(0.85),
        };
        store.insert_aftershock_model(&record).unwrap();

        let latest = store.latest_aftershock_model().unwrap().unwrap();
        assert_eq!(latest.trained_at, 4000);
        assert_eq!(latest.training_samples, 500);
        assert!((latest.auc_roc.unwrap() - 0.85).abs() < 1e-10);
    }

    #[test]
    fn schema_migration_v12_creates_tables() {
        let (store, _dir) = open_test_store();
        let version = store.get_schema_version().unwrap();
        assert!(
            version.version >= 12,
            "schema version should be >= 12, got {}",
            version.version
        );

        // Verify tables exist by querying them
        assert!(store.list_seismograph_metrics(1).is_ok());
        assert!(store.list_community_stability(0).is_ok());
        assert!(store.list_cascades(1).is_ok());
        assert!(store.latest_aftershock_model().is_ok());
    }
}
