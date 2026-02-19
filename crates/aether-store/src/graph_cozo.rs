use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability};

use super::{
    CouplingEdgeRecord, GraphStore, ResolvedEdge, StoreError, SymbolRecord, TestedByRecord,
};

pub struct CozoGraphStore {
    db: DbInstance,
}

impl CozoGraphStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let aether_dir = workspace_root.join(".aether");
        let graph_path = aether_dir.join("graph.db");
        fs::create_dir_all(&aether_dir)?;

        let graph_path_str = graph_path.to_string_lossy().to_string();
        let db = DbInstance::new("sled", &graph_path_str, Default::default())
            .map_err(|err| StoreError::Cozo(err.to_string()))?;
        let store = Self { db };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<(), StoreError> {
        self.ensure_relation(
            r#"
            :create symbols {
                symbol_id: String =>
                qualified_name: String,
                name: String,
                kind: String,
                file_path: String,
                language: String,
                signature_fingerprint: String,
                last_seen_at: Int
            }
            "#,
        )?;
        self.ensure_relation(
            r#"
            :create edges {
                source_id: String,
                target_id: String,
                edge_kind: String =>
                file_path: String
            }
            "#,
        )?;
        self.ensure_relation(
            r#"
            :create co_change_edges {
                file_a: String,
                file_b: String =>
                co_change_count: Int,
                total_commits_a: Int,
                total_commits_b: Int,
                git_coupling: Float,
                static_signal: Float,
                semantic_signal: Float,
                fused_score: Float,
                coupling_type: String,
                last_co_change_commit: String,
                last_co_change_at: Int,
                mined_at: Int
            }
            "#,
        )?;
        self.ensure_relation(
            r#"
            :create tested_by {
                target_file: String,
                test_file: String =>
                intent_count: Int,
                confidence: Float,
                inference_method: String
            }
            "#,
        )?;
        Ok(())
    }

    fn ensure_relation(&self, script: &str) -> Result<(), StoreError> {
        match self.run_script(script, BTreeMap::new(), ScriptMutability::Mutable) {
            Ok(_) => Ok(()),
            Err(StoreError::Cozo(message))
                if message.contains("relation exists")
                    || message.contains("already exists")
                    || message.contains("conflicts with an existing one")
                    || message.contains("Duplicated")
                    || message.contains("duplicate") =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn run_script(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
        mutability: ScriptMutability,
    ) -> Result<NamedRows, StoreError> {
        self.db
            .run_script(script, params, mutability)
            .map_err(|err| StoreError::Cozo(err.to_string()))
    }

    fn row_to_symbol(row: &[DataValue]) -> Result<SymbolRecord, StoreError> {
        if row.len() < 7 {
            return Err(StoreError::Cozo("invalid symbol row shape".to_owned()));
        }

        let last_seen_at = row[6]
            .get_int()
            .ok_or_else(|| StoreError::Cozo("invalid last_seen_at value".to_owned()))?;

        Ok(SymbolRecord {
            id: row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid symbol id value".to_owned()))?
                .to_owned(),
            file_path: row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid file_path value".to_owned()))?
                .to_owned(),
            language: row[2]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid language value".to_owned()))?
                .to_owned(),
            kind: row[3]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid kind value".to_owned()))?
                .to_owned(),
            qualified_name: row[4]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid qualified_name value".to_owned()))?
                .to_owned(),
            signature_fingerprint: row[5]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid signature_fingerprint value".to_owned()))?
                .to_owned(),
            last_seen_at,
        })
    }

    fn symbol_name(qualified_name: &str) -> &str {
        qualified_name
            .rsplit("::")
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(qualified_name)
    }

    fn row_to_coupling_edge(row: &[DataValue]) -> Result<CouplingEdgeRecord, StoreError> {
        if row.len() < 13 {
            return Err(StoreError::Cozo(
                "invalid co_change_edges row shape".to_owned(),
            ));
        }

        let as_f32 = |index: usize, label: &str| -> Result<f32, StoreError> {
            let value = &row[index];
            if let Some(value) = value.get_float() {
                return Ok(value as f32);
            }
            if let Some(value) = value.get_int() {
                return Ok(value as f32);
            }
            Err(StoreError::Cozo(format!("invalid {label} value")))
        };

        Ok(CouplingEdgeRecord {
            file_a: row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid file_a value".to_owned()))?
                .to_owned(),
            file_b: row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid file_b value".to_owned()))?
                .to_owned(),
            co_change_count: row[2]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid co_change_count value".to_owned()))?,
            total_commits_a: row[3]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid total_commits_a value".to_owned()))?,
            total_commits_b: row[4]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid total_commits_b value".to_owned()))?,
            git_coupling: as_f32(5, "git_coupling")?,
            static_signal: as_f32(6, "static_signal")?,
            semantic_signal: as_f32(7, "semantic_signal")?,
            fused_score: as_f32(8, "fused_score")?,
            coupling_type: row[9]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid coupling_type value".to_owned()))?
                .to_owned(),
            last_co_change_commit: row[10]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid last_co_change_commit value".to_owned()))?
                .to_owned(),
            last_co_change_at: row[11]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid last_co_change_at value".to_owned()))?,
            mined_at: row[12]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid mined_at value".to_owned()))?,
        })
    }

    fn row_to_tested_by(row: &[DataValue]) -> Result<TestedByRecord, StoreError> {
        if row.len() < 5 {
            return Err(StoreError::Cozo("invalid tested_by row shape".to_owned()));
        }

        let confidence = if let Some(value) = row[3].get_float() {
            value as f32
        } else if let Some(value) = row[3].get_int() {
            value as f32
        } else {
            return Err(StoreError::Cozo("invalid confidence value".to_owned()));
        };

        Ok(TestedByRecord {
            target_file: row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid target_file value".to_owned()))?
                .to_owned(),
            test_file: row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid test_file value".to_owned()))?
                .to_owned(),
            intent_count: row[2]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid intent_count value".to_owned()))?,
            confidence,
            inference_method: row[4]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid inference_method value".to_owned()))?
                .to_owned(),
        })
    }

    pub fn has_dependency_between_files(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<bool, StoreError> {
        let file_a = file_a.trim();
        let file_b = file_b.trim();
        if file_a.is_empty() || file_b.is_empty() {
            return Ok(false);
        }

        let mut params = BTreeMap::new();
        params.insert("file_a".to_owned(), DataValue::from(file_a.to_owned()));
        params.insert("file_b".to_owned(), DataValue::from(file_b.to_owned()));

        let rows = self.run_script(
            r#"
            ?[source_id] :=
                *edges{source_id, target_id},
                *symbols{symbol_id: source_id, file_path: $file_a},
                *symbols{symbol_id: target_id, file_path: $file_b}
            ?[source_id] :=
                *edges{source_id, target_id},
                *symbols{symbol_id: source_id, file_path: $file_b},
                *symbols{symbol_id: target_id, file_path: $file_a}

            :limit 1
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        Ok(!rows.rows.is_empty())
    }

    pub fn upsert_co_change_edges(&self, records: &[CouplingEdgeRecord]) -> Result<(), StoreError> {
        for record in records {
            let mut params = BTreeMap::new();
            params.insert("file_a".to_owned(), DataValue::from(record.file_a.clone()));
            params.insert("file_b".to_owned(), DataValue::from(record.file_b.clone()));
            params.insert(
                "co_change_count".to_owned(),
                DataValue::from(record.co_change_count),
            );
            params.insert(
                "total_commits_a".to_owned(),
                DataValue::from(record.total_commits_a),
            );
            params.insert(
                "total_commits_b".to_owned(),
                DataValue::from(record.total_commits_b),
            );
            params.insert(
                "git_coupling".to_owned(),
                DataValue::from(record.git_coupling as f64),
            );
            params.insert(
                "static_signal".to_owned(),
                DataValue::from(record.static_signal as f64),
            );
            params.insert(
                "semantic_signal".to_owned(),
                DataValue::from(record.semantic_signal as f64),
            );
            params.insert(
                "fused_score".to_owned(),
                DataValue::from(record.fused_score as f64),
            );
            params.insert(
                "coupling_type".to_owned(),
                DataValue::from(record.coupling_type.clone()),
            );
            params.insert(
                "last_co_change_commit".to_owned(),
                DataValue::from(record.last_co_change_commit.clone()),
            );
            params.insert(
                "last_co_change_at".to_owned(),
                DataValue::from(record.last_co_change_at),
            );
            params.insert("mined_at".to_owned(), DataValue::from(record.mined_at));

            self.run_script(
                r#"
                ?[
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                ] <- [[
                    $file_a,
                    $file_b,
                    $co_change_count,
                    $total_commits_a,
                    $total_commits_b,
                    $git_coupling,
                    $static_signal,
                    $semantic_signal,
                    $fused_score,
                    $coupling_type,
                    $last_co_change_commit,
                    $last_co_change_at,
                    $mined_at
                ]]
                :put co_change_edges {
                    file_a,
                    file_b =>
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                }
                "#,
                params,
                ScriptMutability::Mutable,
            )?;
        }

        Ok(())
    }

    pub fn get_co_change_edge(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<Option<CouplingEdgeRecord>, StoreError> {
        let file_a = file_a.trim();
        let file_b = file_b.trim();
        if file_a.is_empty() || file_b.is_empty() {
            return Ok(None);
        }

        let mut params = BTreeMap::new();
        params.insert("file_a".to_owned(), DataValue::from(file_a.to_owned()));
        params.insert("file_b".to_owned(), DataValue::from(file_b.to_owned()));
        let rows = self.run_script(
            r#"
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                },
                file_a = $file_a,
                file_b = $file_b
            :limit 1
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .first()
            .map(|row| Self::row_to_coupling_edge(row.as_slice()))
            .transpose()
    }

    pub fn list_co_change_edges_for_file(
        &self,
        file_path: &str,
        min_fused_score: f32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "file_path".to_owned(),
            DataValue::from(file_path.to_owned()),
        );
        params.insert(
            "min_fused_score".to_owned(),
            DataValue::from(min_fused_score as f64),
        );
        let rows = self.run_script(
            r#"
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                },
                file_a = $file_path,
                fused_score >= $min_fused_score
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                },
                file_b = $file_path,
                fused_score >= $min_fused_score
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut edges = rows
            .rows
            .iter()
            .map(|row| Self::row_to_coupling_edge(row.as_slice()))
            .collect::<Result<Vec<_>, _>>()?;
        edges.sort_by(|left, right| {
            right
                .fused_score
                .partial_cmp(&left.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file_a.cmp(&right.file_a))
                .then_with(|| left.file_b.cmp(&right.file_b))
        });
        Ok(edges)
    }

    pub fn list_top_co_change_edges(
        &self,
        limit: u32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        let rows = self.run_script(
            r#"
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                }
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut edges = rows
            .rows
            .iter()
            .map(|row| Self::row_to_coupling_edge(row.as_slice()))
            .collect::<Result<Vec<_>, _>>()?;
        edges.sort_by(|left, right| {
            right
                .fused_score
                .partial_cmp(&left.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file_a.cmp(&right.file_a))
                .then_with(|| left.file_b.cmp(&right.file_b))
        });
        edges.truncate(limit.clamp(1, 200) as usize);
        Ok(edges)
    }

    pub fn replace_tested_by_for_test_file(
        &self,
        test_file: &str,
        records: &[TestedByRecord],
    ) -> Result<(), StoreError> {
        let test_file = test_file.trim();
        if test_file.is_empty() {
            return Ok(());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "test_file".to_owned(),
            DataValue::from(test_file.to_owned()),
        );
        self.run_script(
            r#"
            ?[target_file, test_file] :=
                *tested_by{
                    target_file,
                    test_file,
                    intent_count,
                    confidence,
                    inference_method
                },
                test_file = $test_file
            :rm tested_by { target_file, test_file }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        for record in records {
            let mut params = BTreeMap::new();
            params.insert(
                "target_file".to_owned(),
                DataValue::from(record.target_file.clone()),
            );
            params.insert(
                "test_file".to_owned(),
                DataValue::from(record.test_file.clone()),
            );
            params.insert(
                "intent_count".to_owned(),
                DataValue::from(record.intent_count.max(0)),
            );
            params.insert(
                "confidence".to_owned(),
                DataValue::from(record.confidence.clamp(0.0, 1.0) as f64),
            );
            params.insert(
                "inference_method".to_owned(),
                DataValue::from(record.inference_method.clone()),
            );

            self.run_script(
                r#"
                ?[target_file, test_file, intent_count, confidence, inference_method] <- [[
                    $target_file,
                    $test_file,
                    $intent_count,
                    $confidence,
                    $inference_method
                ]]
                :put tested_by {
                    target_file,
                    test_file =>
                    intent_count,
                    confidence,
                    inference_method
                }
                "#,
                params,
                ScriptMutability::Mutable,
            )?;
        }

        Ok(())
    }

    pub fn list_tested_by_for_target_file(
        &self,
        target_file: &str,
    ) -> Result<Vec<TestedByRecord>, StoreError> {
        let target_file = target_file.trim();
        if target_file.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "target_file".to_owned(),
            DataValue::from(target_file.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                target_file,
                test_file,
                intent_count,
                confidence,
                inference_method
            ] :=
                *tested_by{
                    target_file,
                    test_file,
                    intent_count,
                    confidence,
                    inference_method
                },
                target_file = $target_file
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut records = rows
            .rows
            .iter()
            .map(|row| Self::row_to_tested_by(row.as_slice()))
            .collect::<Result<Vec<_>, _>>()?;
        records.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.test_file.cmp(&right.test_file))
        });
        Ok(records)
    }
}

impl GraphStore for CozoGraphStore {
    fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError> {
        let mut params = BTreeMap::new();
        params.insert("symbol_id".to_owned(), DataValue::from(symbol.id.clone()));
        params.insert(
            "qualified_name".to_owned(),
            DataValue::from(symbol.qualified_name.clone()),
        );
        params.insert(
            "name".to_owned(),
            DataValue::from(Self::symbol_name(&symbol.qualified_name).to_owned()),
        );
        params.insert("kind".to_owned(), DataValue::from(symbol.kind.clone()));
        params.insert(
            "file_path".to_owned(),
            DataValue::from(symbol.file_path.clone()),
        );
        params.insert(
            "language".to_owned(),
            DataValue::from(symbol.language.clone()),
        );
        params.insert(
            "signature_fingerprint".to_owned(),
            DataValue::from(symbol.signature_fingerprint.clone()),
        );
        params.insert(
            "last_seen_at".to_owned(),
            DataValue::from(symbol.last_seen_at),
        );

        self.run_script(
            r#"
            ?[
                symbol_id,
                qualified_name,
                name,
                kind,
                file_path,
                language,
                signature_fingerprint,
                last_seen_at
            ] <- [[
                $symbol_id,
                $qualified_name,
                $name,
                $kind,
                $file_path,
                $language,
                $signature_fingerprint,
                $last_seen_at
            ]]
            :put symbols {
                symbol_id =>
                qualified_name,
                name,
                kind,
                file_path,
                language,
                signature_fingerprint,
                last_seen_at
            }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }

    fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError> {
        let mut params = BTreeMap::new();
        params.insert(
            "source_id".to_owned(),
            DataValue::from(edge.source_id.clone()),
        );
        params.insert(
            "target_id".to_owned(),
            DataValue::from(edge.target_id.clone()),
        );
        params.insert(
            "edge_kind".to_owned(),
            DataValue::from(edge.edge_kind.as_str().to_owned()),
        );
        params.insert(
            "file_path".to_owned(),
            DataValue::from(edge.file_path.clone()),
        );

        self.run_script(
            r#"
            ?[source_id, target_id, edge_kind, file_path] <- [[
                $source_id,
                $target_id,
                $edge_kind,
                $file_path
            ]]
            :put edges { source_id, target_id, edge_kind => file_path }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }

    fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "qname".to_owned(),
            DataValue::from(qualified_name.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at
            ] :=
                *edges{source_id: symbol_id, target_id, edge_kind: "calls"},
                *symbols{symbol_id: target_id, qualified_name: $qname},
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .iter()
            .map(|row| Self::row_to_symbol(row.as_slice()))
            .collect()
    }

    fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "source_id".to_owned(),
            DataValue::from(symbol_id.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at
            ] :=
                *edges{source_id: $source_id, target_id: symbol_id, edge_kind: "calls"},
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .iter()
            .map(|row| Self::row_to_symbol(row.as_slice()))
            .collect()
    }

    fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() || depth == 0 {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert("start".to_owned(), DataValue::from(symbol_id.to_owned()));
        params.insert("max_depth".to_owned(), DataValue::from(depth as i64));
        let rows = self.run_script(
            r#"
            reachable[node, depth] :=
                *edges{source_id: $start, target_id: node, edge_kind: "calls"},
                depth = 1
            reachable[node, depth] :=
                reachable[prev, prev_depth],
                prev_depth < $max_depth,
                *edges{source_id: prev, target_id: node, edge_kind: "calls"},
                depth = prev_depth + 1

            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at,
                depth
            ] :=
                reachable[symbol_id, depth],
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order depth, qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut levels = Vec::new();
        let mut seen = HashSet::new();
        for row in &rows.rows {
            if row.len() < 8 {
                return Err(StoreError::Cozo("invalid call chain row shape".to_owned()));
            }
            let record = Self::row_to_symbol(&row[..7])?;
            if !seen.insert(record.id.clone()) {
                continue;
            }
            let depth = row[7]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid depth value".to_owned()))?;
            if depth <= 0 {
                continue;
            }
            let depth_idx = depth as usize - 1;
            while levels.len() <= depth_idx {
                levels.push(Vec::new());
            }
            if let Some(level) = levels.get_mut(depth_idx) {
                level.push(record);
            }
        }

        Ok(levels)
    }

    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "file_path".to_owned(),
            DataValue::from(file_path.to_owned()),
        );
        self.run_script(
            r#"
            ?[source_id, target_id, edge_kind] :=
                *edges{source_id, target_id, edge_kind, file_path: $file_path}

            :rm edges { source_id, target_id, edge_kind }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{Store, SymbolRecord};
    use aether_core::{EdgeKind, SymbolEdge};

    fn symbol(id: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    #[test]
    fn cozo_graph_returns_multi_hop_call_chain() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        let gamma = symbol("sym-gamma", "gamma");
        let delta = symbol("sym-delta", "delta");
        for row in [&alpha, &beta, &gamma, &delta] {
            graph.upsert_symbol_node(row).expect("upsert symbol");
        }

        for edge in [
            ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: gamma.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: gamma.id.clone(),
                target_id: delta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).expect("upsert edge");
        }

        let chain = graph
            .get_call_chain(&alpha.id, 3)
            .expect("get call chain at depth 3");
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0][0].id, beta.id);
        assert_eq!(chain[1][0].id, gamma.id);
        assert_eq!(chain[2][0].id, delta.id);
    }

    #[test]
    fn unresolved_edges_are_skipped_during_sync() {
        let temp = tempdir().expect("tempdir");
        let store = crate::SqliteStore::open(temp.path()).expect("open sqlite store");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        store.upsert_symbol(alpha.clone()).expect("upsert alpha");
        store
            .upsert_edges(&[SymbolEdge {
                source_id: alpha.id.clone(),
                target_qualified_name: "missing::target".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            }])
            .expect("upsert unresolved edge");

        let stats = store
            .sync_graph_for_file(&graph, "src/lib.rs")
            .expect("sync graph for file");
        assert_eq!(stats.resolved_edges, 0);
        assert_eq!(stats.unresolved_edges, 1);

        let deps = graph
            .get_dependencies(&alpha.id)
            .expect("query dependencies after unresolved sync");
        assert!(deps.is_empty());
    }

    #[test]
    fn cozo_graph_stores_and_queries_co_change_edges() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        graph
            .upsert_co_change_edges(&[CouplingEdgeRecord {
                file_a: "src/a.rs".to_owned(),
                file_b: "src/b.rs".to_owned(),
                co_change_count: 4,
                total_commits_a: 6,
                total_commits_b: 7,
                git_coupling: 4.0 / 7.0,
                static_signal: 1.0,
                semantic_signal: 0.7,
                fused_score: 0.5,
                coupling_type: "multi".to_owned(),
                last_co_change_commit: "abc123".to_owned(),
                last_co_change_at: 1_700_000_000,
                mined_at: 1_700_000_100,
            }])
            .expect("upsert co change edge");

        let top = graph
            .list_top_co_change_edges(10)
            .expect("list top co change edges");
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].file_a, "src/a.rs");
        assert_eq!(top[0].file_b, "src/b.rs");

        let direct = graph
            .get_co_change_edge("src/a.rs", "src/b.rs")
            .expect("get direct co change edge");
        assert!(direct.is_some());

        let neighbors = graph
            .list_co_change_edges_for_file("src/a.rs", 0.2)
            .expect("list neighbors");
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].coupling_type, "multi");
    }

    #[test]
    fn cozo_graph_stores_and_queries_tested_by_edges() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        graph
            .replace_tested_by_for_test_file(
                "tests/payment_test.rs",
                &[
                    TestedByRecord {
                        target_file: "src/payment.rs".to_owned(),
                        test_file: "tests/payment_test.rs".to_owned(),
                        intent_count: 3,
                        confidence: 0.9,
                        inference_method: "naming_convention".to_owned(),
                    },
                    TestedByRecord {
                        target_file: "src/ledger.rs".to_owned(),
                        test_file: "tests/payment_test.rs".to_owned(),
                        intent_count: 1,
                        confidence: 0.4,
                        inference_method: "coupling_cross_reference".to_owned(),
                    },
                ],
            )
            .expect("replace tested_by edges");

        let guards = graph
            .list_tested_by_for_target_file("src/payment.rs")
            .expect("list tested_by for target");
        assert_eq!(guards.len(), 1);
        assert_eq!(guards[0].test_file, "tests/payment_test.rs");
        assert_eq!(guards[0].intent_count, 3);
        assert_eq!(guards[0].inference_method, "naming_convention");
    }
}
