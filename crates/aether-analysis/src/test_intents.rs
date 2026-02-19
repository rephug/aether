use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use aether_core::{file_source_id, normalize_path};
use aether_store::{CozoGraphStore, SqliteStore, Store, TestedByRecord};
use serde::{Deserialize, Serialize};

use crate::coupling::AnalysisError;

const NAMING_CONFIDENCE: f32 = 0.9;
const IMPORT_CONFIDENCE: f32 = 0.8;
const SAME_FILE_CONFIDENCE: f32 = 1.0;
const COUPLING_WEIGHT: f32 = 0.7;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestGuard {
    pub test_file: String,
    pub intents: Vec<String>,
    pub confidence: f32,
    pub inference_method: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferredTestTarget {
    pub target_file: String,
    pub test_file: String,
    pub intent_count: i64,
    pub confidence: f32,
    pub inference_method: String,
}

#[derive(Debug, Clone)]
pub struct TestIntentAnalyzer {
    workspace: PathBuf,
}

#[derive(Debug, Clone)]
struct Candidate {
    confidence: f32,
    method: &'static str,
}

impl Candidate {
    fn rank(&self) -> u8 {
        match self.method {
            "same_file" => 4,
            "naming_convention" => 3,
            "import_analysis" => 2,
            "coupling_cross_reference" => 1,
            _ => 0,
        }
    }
}

impl TestIntentAnalyzer {
    pub fn new(workspace: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        Ok(Self {
            workspace: workspace.as_ref().to_path_buf(),
        })
    }

    pub fn refresh_for_test_file(
        &self,
        test_file: &str,
    ) -> Result<Vec<InferredTestTarget>, AnalysisError> {
        let test_file = normalize_repo_path(test_file);
        if test_file.is_empty() {
            return Ok(Vec::new());
        }

        let store = SqliteStore::open(&self.workspace)?;
        let cozo = CozoGraphStore::open(&self.workspace)?;
        let intents = store.list_test_intents_for_file(test_file.as_str())?;
        if intents.is_empty() {
            cozo.replace_tested_by_for_test_file(test_file.as_str(), &[])?;
            return Ok(Vec::new());
        }

        let mut candidates = HashMap::<String, Candidate>::new();

        for target in self.naming_candidates(test_file.as_str()) {
            add_candidate(
                &mut candidates,
                target,
                Candidate {
                    confidence: NAMING_CONFIDENCE,
                    method: "naming_convention",
                },
            );
        }

        let import_targets = self.import_candidates(&store, test_file.as_str())?;
        if !import_targets.is_empty() {
            let confidence = IMPORT_CONFIDENCE / import_targets.len() as f32;
            for target in import_targets {
                add_candidate(
                    &mut candidates,
                    target,
                    Candidate {
                        confidence,
                        method: "import_analysis",
                    },
                );
            }
        }

        for edge in cozo.list_co_change_edges_for_file(test_file.as_str(), 0.0)? {
            let target = if edge.file_a == test_file {
                edge.file_b
            } else {
                edge.file_a
            };
            if is_probably_test_file(target.as_str()) {
                continue;
            }
            add_candidate(
                &mut candidates,
                target,
                Candidate {
                    confidence: (edge.fused_score.clamp(0.0, 1.0) * COUPLING_WEIGHT)
                        .clamp(0.0, 1.0),
                    method: "coupling_cross_reference",
                },
            );
        }

        if self.same_file_rust_test_candidate(test_file.as_str())? {
            add_candidate(
                &mut candidates,
                test_file.clone(),
                Candidate {
                    confidence: SAME_FILE_CONFIDENCE,
                    method: "same_file",
                },
            );
        }

        let intent_count = intents.len() as i64;
        let mut inferred = candidates
            .into_iter()
            .map(|(target_file, candidate)| InferredTestTarget {
                target_file,
                test_file: test_file.clone(),
                intent_count,
                confidence: candidate.confidence,
                inference_method: candidate.method.to_owned(),
            })
            .collect::<Vec<_>>();
        inferred.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.target_file.cmp(&right.target_file))
        });

        let relation_rows = inferred
            .iter()
            .map(|entry| TestedByRecord {
                target_file: entry.target_file.clone(),
                test_file: entry.test_file.clone(),
                intent_count: entry.intent_count,
                confidence: entry.confidence,
                inference_method: entry.inference_method.clone(),
            })
            .collect::<Vec<_>>();
        cozo.replace_tested_by_for_test_file(test_file.as_str(), &relation_rows)?;

        Ok(inferred)
    }

    pub fn list_guards_for_target_file(
        &self,
        target_file: &str,
    ) -> Result<Vec<TestGuard>, AnalysisError> {
        let target_file = normalize_repo_path(target_file);
        if target_file.is_empty() {
            return Ok(Vec::new());
        }

        let store = SqliteStore::open(&self.workspace)?;
        let cozo = CozoGraphStore::open(&self.workspace)?;
        let rows = cozo.list_tested_by_for_target_file(target_file.as_str())?;

        let mut guards = Vec::new();
        for row in rows {
            let intents = store
                .list_test_intents_for_file(row.test_file.as_str())?
                .into_iter()
                .map(|intent| intent.intent_text)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if intents.is_empty() {
                continue;
            }
            guards.push(TestGuard {
                test_file: row.test_file,
                intents,
                confidence: row.confidence.clamp(0.0, 1.0),
                inference_method: row.inference_method,
            });
        }

        guards.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.test_file.cmp(&right.test_file))
        });

        Ok(guards)
    }

    fn naming_candidates(&self, test_file: &str) -> Vec<String> {
        let mut candidates = BTreeSet::new();
        let normalized = normalize_repo_path(test_file);
        let file_name = Path::new(normalized.as_str())
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();

        for pattern in [".test.", ".spec."] {
            if file_name.contains(pattern) {
                let candidate = normalized.replacen(pattern, ".", 1);
                if let Some(existing) = self.ensure_existing_repo_file(candidate.as_str()) {
                    candidates.insert(existing);
                }
            }
        }

        if normalized.contains("/__tests__/") {
            let candidate = normalized.replace("/__tests__/", "/");
            if let Some(existing) = self.ensure_existing_repo_file(candidate.as_str()) {
                candidates.insert(existing);
            }
        }

        if let Some((root, test_tail)) = split_tests_dir(normalized.as_str()) {
            let ext = Path::new(test_tail.as_str())
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let stem = Path::new(test_tail.as_str())
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let base = stem
                .strip_suffix("_tests")
                .or_else(|| stem.strip_suffix("_test"))
                .or_else(|| stem.strip_prefix("test_"))
                .unwrap_or(stem);

            if !base.trim().is_empty() && !ext.trim().is_empty() {
                let prefix = if root.is_empty() {
                    "src".to_owned()
                } else {
                    format!("{root}/src")
                };
                let candidate = format!("{prefix}/{base}.{ext}");
                if let Some(existing) = self.ensure_existing_repo_file(candidate.as_str()) {
                    candidates.insert(existing);
                }
            }
        }

        candidates.into_iter().collect()
    }

    fn import_candidates(
        &self,
        store: &SqliteStore,
        test_file: &str,
    ) -> Result<Vec<String>, AnalysisError> {
        let mut targets = BTreeSet::new();
        let deps = store.get_dependencies(file_source_id(test_file).as_str())?;
        for dep in deps {
            for target in self.resolve_import_target(test_file, dep.target_qualified_name.as_str())
            {
                if is_probably_test_file(target.as_str()) {
                    continue;
                }
                targets.insert(target);
            }
        }
        Ok(targets.into_iter().collect())
    }

    fn resolve_import_target(&self, test_file: &str, import: &str) -> Vec<String> {
        let import = import.trim().trim_matches('"').trim_matches('\'');
        if import.is_empty() {
            return Vec::new();
        }

        let mut candidates = BTreeSet::new();
        let file_path = Path::new(test_file);
        let ext = file_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let extension_candidates = match ext {
            "rs" => vec!["rs"],
            "py" | "pyi" => vec!["py", "pyi"],
            _ => vec!["ts", "tsx", "js", "jsx"],
        };

        let parent = self
            .workspace
            .join(file_path.parent().unwrap_or_else(|| Path::new("")));
        if import.starts_with("./") || import.starts_with("../") {
            let base = parent.join(import);
            add_existing_path_candidates(
                &self.workspace,
                &base,
                extension_candidates.as_slice(),
                &mut candidates,
            );
        }

        if import.starts_with('/') {
            let absolute_repo_path = import.trim_start_matches('/');
            if let Some(existing) = self.ensure_existing_repo_file(absolute_repo_path) {
                candidates.insert(existing);
            }
        }

        if import.starts_with("crate::")
            || import.starts_with("self::")
            || import.starts_with("super::")
        {
            let root = infer_project_root(test_file);
            let module = import
                .trim_start_matches("crate::")
                .trim_start_matches("self::")
                .trim_start_matches("super::")
                .replace("::", "/");
            if !module.is_empty() {
                let src_root = if root.is_empty() {
                    self.workspace.join("src")
                } else {
                    self.workspace.join(root).join("src")
                };
                let base = src_root.join(module);
                add_existing_path_candidates(
                    &self.workspace,
                    &base,
                    &["rs", "ts", "tsx", "js", "jsx", "py", "pyi"],
                    &mut candidates,
                );
            }
        }

        if import.contains('.') && !import.contains('/') {
            let module_path = import.replace('.', "/");
            let base = self.workspace.join(module_path);
            add_existing_path_candidates(&self.workspace, &base, &["py", "pyi"], &mut candidates);
        }

        if !import.starts_with('.') && !import.contains("::") {
            let base = self.workspace.join(import);
            add_existing_path_candidates(
                &self.workspace,
                &base,
                &["rs", "ts", "tsx", "js", "jsx", "py", "pyi"],
                &mut candidates,
            );
        }

        candidates.into_iter().collect()
    }

    fn same_file_rust_test_candidate(&self, test_file: &str) -> Result<bool, AnalysisError> {
        if !test_file.ends_with(".rs") {
            return Ok(false);
        }

        let path = self.workspace.join(test_file);
        if !path.exists() {
            return Ok(false);
        }

        let source = fs::read_to_string(path)?;
        Ok(source.contains("#[cfg(test)]"))
    }

    fn ensure_existing_repo_file(&self, candidate: &str) -> Option<String> {
        let absolute = self.workspace.join(candidate);
        if !absolute.is_file() {
            return None;
        }
        to_workspace_relative(self.workspace.as_path(), absolute.as_path())
    }
}

fn add_candidate(map: &mut HashMap<String, Candidate>, target_file: String, candidate: Candidate) {
    let target_file = normalize_repo_path(target_file.as_str());
    if target_file.is_empty() {
        return;
    }

    match map.get(target_file.as_str()) {
        Some(existing)
            if existing.confidence > candidate.confidence
                || (existing.confidence == candidate.confidence
                    && existing.rank() >= candidate.rank()) => {}
        _ => {
            map.insert(target_file, candidate);
        }
    }
}

fn add_existing_path_candidates(
    workspace: &Path,
    base: &Path,
    extensions: &[&str],
    out: &mut BTreeSet<String>,
) {
    if base.is_file() {
        if let Some(relative) = to_workspace_relative(workspace, base) {
            out.insert(relative);
        }
        return;
    }

    if base.extension().is_some() && base.is_file() {
        if let Some(relative) = to_workspace_relative(workspace, base) {
            out.insert(relative);
        }
        return;
    }

    for ext in extensions {
        let with_ext = base.with_extension(ext);
        if with_ext.is_file()
            && let Some(relative) = to_workspace_relative(workspace, with_ext.as_path())
        {
            out.insert(relative);
        }
    }

    for ext in extensions {
        let index = base.join(format!("index.{ext}"));
        if index.is_file()
            && let Some(relative) = to_workspace_relative(workspace, index.as_path())
        {
            out.insert(relative);
        }

        let module = base.join(format!("mod.{ext}"));
        if module.is_file()
            && let Some(relative) = to_workspace_relative(workspace, module.as_path())
        {
            out.insert(relative);
        }
    }

    let package_init = base.join("__init__.py");
    if package_init.is_file()
        && let Some(relative) = to_workspace_relative(workspace, package_init.as_path())
    {
        out.insert(relative);
    }
}

fn to_workspace_relative(workspace: &Path, path: &Path) -> Option<String> {
    let workspace = workspace.canonicalize().ok()?;
    let resolved = path.canonicalize().ok()?;
    let relative = resolved.strip_prefix(workspace).ok()?;
    Some(normalize_path(&relative.to_string_lossy()))
}

fn infer_project_root(file_path: &str) -> String {
    if let Some((root, _)) = split_tests_dir(file_path) {
        return root;
    }
    if let Some((root, _)) = file_path.split_once("/src/") {
        return root.to_owned();
    }
    String::new()
}

fn split_tests_dir(path: &str) -> Option<(String, String)> {
    if let Some(rest) = path.strip_prefix("tests/") {
        return Some((String::new(), rest.to_owned()));
    }

    let (prefix, rest) = path.split_once("/tests/")?;
    Some((prefix.to_owned(), rest.to_owned()))
}

fn normalize_repo_path(path: &str) -> String {
    normalize_path(path.trim())
}

fn is_probably_test_file(path: &str) -> bool {
    let normalized = normalize_repo_path(path);
    if normalized.contains("/tests/") || normalized.contains("/__tests__/") {
        return true;
    }

    let file_name = Path::new(normalized.as_str())
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    file_name.starts_with("test_")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_tests.rs")
        || file_name.contains(".test.")
        || file_name.contains(".spec.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::{EdgeKind, SymbolEdge, content_hash};
    use aether_store::{CouplingEdgeRecord, TestIntentRecord};
    use tempfile::tempdir;

    fn test_intent(file_path: &str, test_name: &str, intent: &str) -> TestIntentRecord {
        TestIntentRecord {
            intent_id: content_hash(format!("{file_path}\n{test_name}\n{intent}").as_str()),
            file_path: file_path.to_owned(),
            test_name: test_name.to_owned(),
            intent_text: intent.to_owned(),
            group_label: None,
            language: "rust".to_owned(),
            symbol_id: None,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_001,
        }
    }

    #[test]
    fn infers_target_file_from_naming_convention() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::create_dir_all(temp.path().join("tests")).expect("create tests");
        fs::write(temp.path().join("src/payment.rs"), "fn charge() {}\n").expect("write source");
        fs::write(
            temp.path().join("tests/payment_test.rs"),
            "#[test]\nfn test_charge() {}\n",
        )
        .expect("write test");

        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .replace_test_intents_for_file(
                "tests/payment_test.rs",
                &[test_intent(
                    "tests/payment_test.rs",
                    "test_charge",
                    "charges correctly",
                )],
            )
            .expect("write intents");

        let analyzer = TestIntentAnalyzer::new(temp.path()).expect("create analyzer");
        let links = analyzer
            .refresh_for_test_file("tests/payment_test.rs")
            .expect("refresh links");
        assert!(
            links.iter().any(|link| {
                link.target_file == "src/payment.rs"
                    && link.inference_method == "naming_convention"
                    && (link.confidence - 0.9).abs() < 1e-6
            }),
            "expected naming-convention link to src/payment.rs"
        );
    }

    #[test]
    fn infers_target_file_from_import_edges_with_split_confidence() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::create_dir_all(temp.path().join("tests")).expect("create tests");
        fs::write(temp.path().join("src/payment.ts"), "export const x = 1;\n")
            .expect("write payment source");
        fs::write(temp.path().join("src/ledger.ts"), "export const y = 2;\n")
            .expect("write ledger source");
        fs::write(
            temp.path().join("tests/payment.test.ts"),
            "import { x } from \"../src/payment\";\nimport { y } from \"../src/ledger\";\n",
        )
        .expect("write test file");

        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .replace_test_intents_for_file(
                "tests/payment.test.ts",
                &[test_intent(
                    "tests/payment.test.ts",
                    "test",
                    "handles payment flows",
                )],
            )
            .expect("write intents");
        store
            .upsert_edges(&[
                SymbolEdge {
                    source_id: file_source_id("tests/payment.test.ts"),
                    target_qualified_name: "../src/payment".to_owned(),
                    edge_kind: EdgeKind::DependsOn,
                    file_path: "tests/payment.test.ts".to_owned(),
                },
                SymbolEdge {
                    source_id: file_source_id("tests/payment.test.ts"),
                    target_qualified_name: "../src/ledger".to_owned(),
                    edge_kind: EdgeKind::DependsOn,
                    file_path: "tests/payment.test.ts".to_owned(),
                },
            ])
            .expect("upsert dependency edges");

        let analyzer = TestIntentAnalyzer::new(temp.path()).expect("create analyzer");
        let links = analyzer
            .refresh_for_test_file("tests/payment.test.ts")
            .expect("refresh links");
        assert!(
            links.iter().any(|link| {
                link.target_file == "src/payment.ts"
                    && link.inference_method == "import_analysis"
                    && (link.confidence - 0.4).abs() < 1e-6
            }),
            "expected import-analysis link to src/payment.ts with split confidence"
        );
        assert!(
            links.iter().any(|link| {
                link.target_file == "src/ledger.ts"
                    && link.inference_method == "import_analysis"
                    && (link.confidence - 0.4).abs() < 1e-6
            }),
            "expected import-analysis link to src/ledger.ts with split confidence"
        );
    }

    #[test]
    fn guards_include_intents_for_target_file() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::create_dir_all(temp.path().join("tests")).expect("create tests");
        fs::write(temp.path().join("src/payment.rs"), "fn charge() {}\n").expect("write source");

        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .replace_test_intents_for_file(
                "tests/payment_test.rs",
                &[
                    test_intent("tests/payment_test.rs", "test_charge", "charges correctly"),
                    test_intent(
                        "tests/payment_test.rs",
                        "test_errors",
                        "handles invalid input",
                    ),
                ],
            )
            .expect("write intents");

        let cozo = CozoGraphStore::open(temp.path()).expect("open cozo");
        cozo.replace_tested_by_for_test_file(
            "tests/payment_test.rs",
            &[TestedByRecord {
                target_file: "src/payment.rs".to_owned(),
                test_file: "tests/payment_test.rs".to_owned(),
                intent_count: 2,
                confidence: 0.9,
                inference_method: "naming_convention".to_owned(),
            }],
        )
        .expect("write tested_by");
        drop(cozo);
        drop(store);

        let analyzer = TestIntentAnalyzer::new(temp.path()).expect("create analyzer");
        let guards = analyzer
            .list_guards_for_target_file("src/payment.rs")
            .expect("query guards");
        assert_eq!(guards.len(), 1);
        assert_eq!(guards[0].test_file, "tests/payment_test.rs");
        assert!(guards[0].intents.contains(&"charges correctly".to_owned()));
        assert!(
            guards[0]
                .intents
                .contains(&"handles invalid input".to_owned())
        );
    }

    #[test]
    fn coupling_signal_can_seed_targets_when_available() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::create_dir_all(temp.path().join("tests")).expect("create tests");
        fs::write(temp.path().join("src/payment.rs"), "fn charge() {}\n").expect("write source");

        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .replace_test_intents_for_file(
                "tests/payment_cases.rs",
                &[test_intent(
                    "tests/payment_cases.rs",
                    "test_charge",
                    "charges correctly",
                )],
            )
            .expect("write intents");

        let cozo = CozoGraphStore::open(temp.path()).expect("open cozo");
        cozo.upsert_co_change_edges(&[CouplingEdgeRecord {
            file_a: "src/payment.rs".to_owned(),
            file_b: "tests/payment_cases.rs".to_owned(),
            co_change_count: 5,
            total_commits_a: 10,
            total_commits_b: 7,
            git_coupling: 0.5,
            static_signal: 0.0,
            semantic_signal: 0.0,
            fused_score: 0.6,
            coupling_type: "temporal".to_owned(),
            last_co_change_commit: "abc123".to_owned(),
            last_co_change_at: 1_700_000_000,
            mined_at: 1_700_000_100,
        }])
        .expect("upsert coupling edge");
        drop(cozo);
        drop(store);

        let analyzer = TestIntentAnalyzer::new(temp.path()).expect("create analyzer");
        let links = analyzer
            .refresh_for_test_file("tests/payment_cases.rs")
            .expect("refresh links");
        assert!(
            links.iter().any(|entry| {
                entry.target_file == "src/payment.rs"
                    && entry.inference_method == "coupling_cross_reference"
                    && (entry.confidence - 0.42).abs() < 1e-6
            }),
            "expected coupling-derived target with weighted confidence"
        );
    }
}
