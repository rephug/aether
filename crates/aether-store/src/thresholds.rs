use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdCalibrationRecord {
    pub language: String,
    pub threshold: f32,
    pub sample_size: i64,
    pub provider: String,
    pub model: String,
    pub calibrated_at: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationEmbeddingRecord {
    pub symbol_id: String,
    pub file_path: String,
    pub language: String,
    pub embedding: Vec<f32>,
}

impl SqliteStore {
    pub(crate) fn store_upsert_threshold_calibration(
        &self,
        record: ThresholdCalibrationRecord,
    ) -> Result<(), StoreError> {
        let language = record.language.trim().to_ascii_lowercase();
        if language.is_empty() {
            return Ok(());
        }

        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO threshold_calibration (
                language, threshold, sample_size, provider, model, calibrated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(language) DO UPDATE SET
                threshold = excluded.threshold,
                sample_size = excluded.sample_size,
                provider = excluded.provider,
                model = excluded.model,
                calibrated_at = excluded.calibrated_at
            "#,
            params![
                language,
                record.threshold,
                record.sample_size,
                record.provider,
                record.model,
                record.calibrated_at
            ],
        )?;

        Ok(())
    }
    pub(crate) fn store_get_threshold_calibration(
        &self,
        language: &str,
    ) -> Result<Option<ThresholdCalibrationRecord>, StoreError> {
        let language = language.trim().to_ascii_lowercase();
        if language.is_empty() {
            return Ok(None);
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT language, threshold, sample_size, provider, model, calibrated_at
            FROM threshold_calibration
            WHERE language = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![language], |row| {
                Ok(ThresholdCalibrationRecord {
                    language: row.get(0)?,
                    threshold: row.get(1)?,
                    sample_size: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    calibrated_at: row.get(5)?,
                })
            })
            .optional()?;

        Ok(record)
    }
    pub(crate) fn store_list_threshold_calibrations(
        &self,
    ) -> Result<Vec<ThresholdCalibrationRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT language, threshold, sample_size, provider, model, calibrated_at
            FROM threshold_calibration
            ORDER BY language ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ThresholdCalibrationRecord {
                language: row.get(0)?,
                threshold: row.get(1)?,
                sample_size: row.get(2)?,
                provider: row.get(3)?,
                model: row.get(4)?,
                calibrated_at: row.get(5)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
    pub(crate) fn store_list_embeddings_for_provider_model(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Vec<CalibrationEmbeddingRecord>, StoreError> {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT e.symbol_id, s.file_path, s.language, e.embedding_json
            FROM sir_embeddings e
            JOIN symbols s ON s.id = e.symbol_id
            WHERE e.provider = ?1
              AND e.model = ?2
            ORDER BY s.language ASC, s.file_path ASC, e.symbol_id ASC
            "#,
        )?;

        let rows = stmt.query_map(params![provider, model], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut records = Vec::new();
        for row in rows {
            let (symbol_id, file_path, language, embedding_json) = row?;
            let embedding = json_from_str::<Vec<f32>>(&embedding_json)?;
            records.push(CalibrationEmbeddingRecord {
                symbol_id,
                file_path,
                language: language.trim().to_ascii_lowercase(),
                embedding,
            });
        }

        Ok(records)
    }
}
