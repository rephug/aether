//! Workspace-wide (global) Louvain communities as drift and health scoring use them:
//! test symbols left out and the planner's resolution, so that files are judged by
//! how production code clusters (decision #89.1).

use std::collections::HashSet;

use aether_core::normalize_path;
use aether_store::{SqliteStore, SurrealGraphStore};

use crate::coupling::AnalysisError;
use crate::test_intents::is_csharp_test_path;

/// Whether a symbol is test code, judged from its name and path alone (no per-symbol
/// store lookups, so it stays cheap on large workspaces):
/// - the leaf name starts with `test_`;
/// - a module segment of the qualified name is `tests` or `test`, which covers Rust's
///   inline `#[cfg(test)] mod tests` (qualified names are file-relative, `tests::…`);
/// - the file sits under a `tests/` or `__tests__/` directory, or is a `tests.rs`
///   module file;
/// - the file name follows a common test convention: `test_*.py`, `*_test.py`,
///   `*_test.go`, or contains `.test.` / `.spec.` (the JavaScript and TypeScript
///   conventions, applied to any language);
/// - a C# file is named `*Tests.cs` / `*Test.cs` or sits in a `*.Tests`-style project.
///
/// Being name-based, it misses test modules with other names (`mod parser_tests`) and
/// takes production functions named `test_*` for tests, as the planner's rule does.
pub fn is_test_symbol(qualified_name: &str, file_path: &str) -> bool {
    let mut segments = qualified_name.split("::").collect::<Vec<_>>();
    let leaf = segments.pop().unwrap_or(qualified_name);
    if leaf.to_ascii_lowercase().starts_with("test_") {
        return true;
    }
    if segments
        .iter()
        .any(|segment| matches!(*segment, "tests" | "test"))
    {
        return true;
    }

    let path = normalize_path(file_path);
    if is_csharp_test_path(&path) {
        return true;
    }
    let path = path.to_ascii_lowercase();
    let in_test_dir =
        |dir: &str| path.starts_with(&format!("{dir}/")) || path.contains(&format!("/{dir}/"));
    if in_test_dir("tests") || in_test_dir("__tests__") {
        return true;
    }
    let file_name = path.rsplit('/').next().unwrap_or(path.as_str());
    file_name == "tests.rs"
        || (file_name.starts_with("test_") && file_name.ends_with(".py"))
        || file_name.ends_with("_test.py")
        || file_name.ends_with("_test.go")
        || [".test.", ".spec."]
            .iter()
            .any(|marker| file_name.contains(marker))
}

/// Ids of every indexed symbol that [`is_test_symbol`] classifies as test code.
pub fn test_symbol_ids(store: &SqliteStore) -> Result<HashSet<String>, AnalysisError> {
    Ok(store
        .list_symbol_names_and_paths()?
        .into_iter()
        .filter(|(_, qualified_name, file_path)| is_test_symbol(qualified_name, file_path))
        .map(|(id, _, _)| id)
        .collect())
}

/// Global Louvain community assignments with test symbols excluded, at `resolution`
/// (normally `planner.community_resolution`).
pub async fn production_community_assignments(
    store: &SqliteStore,
    graph: &SurrealGraphStore,
    resolution: f64,
) -> Result<Vec<(String, i64)>, AnalysisError> {
    let exclude = test_symbol_ids(store)?;
    Ok(graph
        .list_louvain_communities_filtered(resolution, &exclude)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::is_test_symbol;

    #[test]
    fn recognizes_test_symbols_by_name_and_path() {
        for (qualified_name, file_path) in [
            ("test_parses_config", "src/config.rs"),
            ("tests::rejects_empty_input", "crates/a/src/lib.rs"),
            ("parser::tests::helpers::fixture", "crates/a/src/parser.rs"),
            ("module::test::case", "src/lib.rs"),
            ("builds_index", "crates/a/tests/integration.rs"),
            ("builds_index", "tests/integration.rs"),
            ("empty_app", "crates/dashboard/src/tests.rs"),
            ("check_totals", "pkg/test_billing.py"),
            ("check_totals", "pkg/billing_test.py"),
            ("TestTotals", "pkg/billing_test.go"),
            ("renders", "web/src/App.test.tsx"),
            ("renders", "web/src/App.spec.ts"),
            ("renders", "web/src/__tests__/App.tsx"),
            ("renders", "Web\\Src\\__Tests__\\App.tsx"),
            (
                "LoggerTests::WritesEvent",
                "test/Serilog.Tests/Core/LoggerTests.cs",
            ),
            (
                "PaymentTests::Charges",
                "Acme.Billing.Tests/PaymentTests.cs",
            ),
            ("Fixture::Build", "src/Acme.Billing.UnitTests/Fixture.cs"),
        ] {
            assert!(
                is_test_symbol(qualified_name, file_path),
                "{qualified_name} in {file_path}"
            );
        }
    }

    #[test]
    fn leaves_production_symbols_alone() {
        for (qualified_name, file_path) in [
            ("parse_config", "src/config.rs"),
            ("Config::testing_mode", "src/config.rs"),
            ("testing::Harness::run", "src/testing.rs"),
            ("contest::score", "src/contest.rs"),
            ("latest_tests_summary", "src/report.rs"),
            ("attestation::verify", "src/attestation.rs"),
            ("load", "src/tests_support.rs"),
            ("load", "pkg/testing.py"),
            ("render", "web/src/latest.ts"),
            ("Contest::Score", "src/Acme/Contest.cs"),
            ("Support::Load", "src/Acme.Tests.Support/Support.cs"),
            ("load", "src/Acme.Tests/loader.py"),
        ] {
            assert!(
                !is_test_symbol(qualified_name, file_path),
                "{qualified_name} in {file_path}"
            );
        }
    }
}
