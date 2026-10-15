//! Whether code is test code, judged from its file path and qualified name alone.
//!
//! Every consumer shares this rule: global communities leave test symbols out, the per-file
//! community count and the split planner skip them (the planner also treats symbols that a
//! test intent names as tests), test intents don't count the test files a test calls into as
//! tested code, and the dashboard files them under its Tests layer. It needs no store
//! lookups, so it stays cheap on large workspaces. Being name-based, it misses tests kept
//! under other names (`mod parser_checks`, a `test/` package below the workspace root) and
//! takes production code named like tests (`fn test_connection`, Jinja's `tests.py`) for
//! tests.

/// Suffixes .NET solutions give the project that tests another one.
pub const CSHARP_TEST_PROJECT_SUFFIXES: [&str; 4] =
    [".UnitTests", ".IntegrationTests", ".Tests", ".Test"];

/// Whether a file is test code by its path:
/// - it sits under a test directory: `tests/` or `__tests__/` anywhere, `test/` at the top
///   of the workspace, or a Maven or Gradle test source set (`src/test/`,
///   `src/testFixtures/`, `src/integrationTest/`, `src/androidTest/`). A `test` directory
///   elsewhere is often a production package (Django's `django/test/`, Go's
///   `internal/test/`), and nothing under `src/main/`, Maven's and Gradle's production
///   sources, is a test directory;
/// - its name follows its language's test convention: Python's `test_*.py`, `*_test.py`,
///   `*_tests.py`, Django's `tests.py` and pytest's `conftest.py`; Go's `*_test.go`; Rust's
///   `tests.rs` and `*_tests.rs` module files; Deno's `*_test.ts`; and a `.test.` or `.spec.`
///   infix in any language (`App.test.tsx`, `api.spec.ts`);
/// - it is a C# file named `*Tests.cs` or `*Test.cs`, or in a `*.Tests`, `*.Test`,
///   `*.UnitTests` or `*.IntegrationTests` project.
///
/// Names are compared without case, except C#'s PascalCase test names and Gradle's
/// camel-case source sets.
pub fn is_test_file(path: &str) -> bool {
    let path = crate::normalize_path(path.trim());
    if is_csharp_test_file(&path) {
        return true;
    }
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>();
    let Some((file_name, directories)) = segments.split_last() else {
        return false;
    };
    if in_test_directory(directories) {
        return true;
    }

    let file_name = file_name.to_ascii_lowercase();
    if file_name.contains(".test.") || file_name.contains(".spec.") {
        return true;
    }
    let stem = file_name
        .split_once('.')
        .map_or(file_name.as_str(), |(stem, _)| stem);
    let extension = file_name
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension);
    match extension {
        "py" => {
            stem.starts_with("test_")
                || stem.ends_with("_test")
                || stem.ends_with("_tests")
                || stem == "tests"
                || stem == "conftest"
        }
        "go" => stem.ends_with("_test"),
        "rs" => stem == "tests" || stem.ends_with("_tests"),
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => stem.ends_with("_test"),
        _ => false,
    }
}

/// Whether a symbol is test code: its file is ([`is_test_file`]), its leaf name starts
/// with `test_`, or a module segment of its qualified name is `tests` or `test`. Rust
/// qualified names are file-relative, so the last covers Rust's inline
/// `#[cfg(test)] mod tests` (`tests::rejects_empty_input`). The `test_` prefix keeps its
/// case, so constants like `TEST_RUNNER` stay production code.
pub fn is_test_symbol(qualified_name: &str, file_path: &str) -> bool {
    let mut segments = qualified_name.split("::").collect::<Vec<_>>();
    let leaf = segments.pop().unwrap_or(qualified_name);
    leaf.starts_with("test_")
        || segments
            .iter()
            .any(|segment| matches!(*segment, "tests" | "test"))
        || is_test_file(file_path)
}

/// Whether directories, outermost first, hold test code.
fn in_test_directory(directories: &[&str]) -> bool {
    let mut parent = String::new();
    for (index, directory) in directories.iter().enumerate() {
        let name = directory.to_ascii_lowercase();
        let in_src = parent == "src";
        if in_src && name == "main" {
            return false;
        }
        let test_directory = match name.as_str() {
            "tests" | "__tests__" => true,
            "test" => index == 0 || in_src,
            "testfixtures" => in_src,
            _ => in_src && directory.len() > "Test".len() && directory.ends_with("Test"),
        };
        if test_directory {
            return true;
        }
        parent = name;
    }
    false
}

/// Whether a normalized path is a C# test file: a `*Tests.cs` or `*Test.cs` file, or a file
/// in a `*.Tests`, `*.Test`, `*.UnitTests` or `*.IntegrationTests` project.
fn is_csharp_test_file(path: &str) -> bool {
    path.ends_with(".cs")
        && (path.ends_with("Tests.cs")
            || path.ends_with("Test.cs")
            || path.split('/').any(|segment| {
                CSHARP_TEST_PROJECT_SUFFIXES
                    .iter()
                    .any(|suffix| segment.len() > suffix.len() && segment.ends_with(suffix))
            }))
}

#[cfg(test)]
mod tests {
    use super::{is_test_file, is_test_symbol};

    /// `(qualified name, file path, is test code)`, by language.
    const CASES: &[(&str, &str, bool)] = &[
        // Rust: integration tests, inline test modules, test module files.
        ("builds_index", "tests/integration.rs", true),
        ("builds_index", "crates/a/tests/integration.rs", true),
        ("tests::rejects_empty_input", "crates/a/src/lib.rs", true),
        (
            "parser::tests::helpers::fixture",
            "crates/a/src/parser.rs",
            true,
        ),
        ("module::test::case", "src/lib.rs", true),
        ("test_parses_config", "src/config.rs", true),
        ("empty_app", "crates/dashboard/src/tests.rs", true),
        ("fixture", "crates/a/src/parser_tests.rs", true),
        ("helpers::fixture", "src/test/mod.rs", true),
        ("parse_config", "src/config.rs", false),
        ("Config::testing_mode", "src/config.rs", false),
        ("testing::Harness::run", "src/testing.rs", false),
        ("contest::score", "src/contest.rs", false),
        ("latest_tests_summary", "src/report.rs", false),
        ("attestation::verify", "src/attestation.rs", false),
        ("load", "src/tests_support.rs", false),
        ("run", "src/commands/test.rs", false),
        ("exec", "src/commands/test/mod.rs", false),
        ("run_tests", "library/test/src/lib.rs", false),
        ("run_tests", "src/cargo/ops/cargo_test.rs", false),
        (
            "TestIntentAnalyzer::new",
            "crates/analysis/src/test_intents.rs",
            false,
        ),
        // Python: pytest and unittest layouts, Django's `tests.py`, pytest fixtures.
        (
            "tests.test_orders::test_place",
            "tests/test_orders.py",
            true,
        ),
        ("tests.helpers::make_order", "tests/helpers.py", true),
        ("test.helpers::make_order", "test/helpers.py", true),
        (
            "pkg.tests.factories::OrderFactory",
            "pkg/tests/factories.py",
            true,
        ),
        ("tests.conftest::client", "tests/conftest.py", true),
        ("conftest::client", "conftest.py", true),
        ("src.pkg.conftest::db", "src/pkg/conftest.py", true),
        (
            "pkg.test_billing::check_totals",
            "pkg/test_billing.py",
            true,
        ),
        (
            "pkg.billing_test::check_totals",
            "pkg/billing_test.py",
            true,
        ),
        (
            "pkg.billing_tests::check_totals",
            "pkg/billing_tests.py",
            true,
        ),
        ("shop.tests::OrderTests::setUp", "shop/tests.py", true),
        ("pkg.orders::TestOrders::test_total", "pkg/orders.py", true),
        ("pkg.orders::place", "pkg/orders.py", false),
        ("pkg.testing::load", "pkg/testing.py", false),
        ("pkg.contest::score", "pkg/contest.py", false),
        ("pkg.commands.test::run", "pkg/commands/test.py", false),
        ("django.test.client::Client", "django/test/client.py", false),
        ("pkg.settings::TEST_RUNNER", "pkg/settings.py", false),
        (
            "src.Acme.Tests.loader::load",
            "src/Acme.Tests/loader.py",
            false,
        ),
        // TypeScript and JavaScript: `.test.` / `.spec.` files, `__tests__/`, `test/`.
        ("renders", "web/src/App.test.tsx", true),
        ("renders", "web/src/App.spec.ts", true),
        ("renders", "web/src/__tests__/App.tsx", true),
        ("renders", "Web\\Src\\__Tests__\\App.tsx", true),
        ("setup", "test/setup.js", true),
        ("parses", "src/utils_test.ts", true),
        ("render", "web/src/latest.ts", false),
        ("render", "web/src/testimonials.tsx", false),
        ("render", "web/src/test_ids.ts", false),
        // Go: `_test.go` files, and a `test/` directory of integration tests.
        ("TestTotals", "pkg/billing_test.go", true),
        ("harness", "test/e2e/harness.go", true),
        ("Total", "pkg/billing.go", false),
        ("Run", "src/cmd/go/internal/test/test.go", false),
        ("Helpers", "pkg/tests.go", false),
        ("Fixture", "pkg/billing_tests.go", false),
        ("Score", "pkg/contest/contest.go", false),
        // Java: Maven's and Gradle's `src/test`, but not a `test` package under `src/main`.
        (
            "PaymentServiceTest::chargesCard",
            "billing/src/test/java/com/acme/PaymentServiceTest.java",
            true,
        ),
        (
            "Fixtures::order",
            "src/test/java/com/acme/Fixtures.java",
            true,
        ),
        (
            "Orders::sample",
            "app/src/testFixtures/java/Orders.java",
            true,
        ),
        (
            "CheckoutIT::runs",
            "app/src/integrationTest/java/CheckoutIT.java",
            true,
        ),
        (
            "MainActivityTest::opens",
            "app/src/androidTest/java/Main.java",
            true,
        ),
        (
            "Probe::check",
            "src/main/java/com/acme/tests/Probe.java",
            false,
        ),
        (
            "Contest::score",
            "src/main/java/com/acme/test/Contest.java",
            false,
        ),
        (
            "PaymentService::charge",
            "src/main/java/com/acme/PaymentService.java",
            false,
        ),
        // C#: `*Tests.cs` files and `.Tests` projects.
        (
            "LoggerTests::WritesEvent",
            "test/Serilog.Tests/Core/LoggerTests.cs",
            true,
        ),
        (
            "PaymentTests::Charges",
            "Acme.Billing.Tests/PaymentTests.cs",
            true,
        ),
        ("Helpers::Build", "Acme.Billing.Tests/Helpers.cs", true),
        (
            "Fixture::Build",
            "src/Acme.Billing.UnitTests/Fixture.cs",
            true,
        ),
        ("Contest::Score", "src/Acme/Contest.cs", false),
        ("Contest::Score", "src/Acme.Billing/Contest.cs", false),
        ("Support::Load", "src/Acme.Tests.Support/Support.cs", false),
    ];

    #[test]
    fn classifies_test_code_across_languages() {
        let wrong = CASES
            .iter()
            .filter(|(qualified_name, file_path, expected)| {
                is_test_symbol(qualified_name, file_path) != *expected
            })
            .collect::<Vec<_>>();
        assert!(wrong.is_empty(), "misclassified: {wrong:#?}");
    }

    #[test]
    fn a_test_file_makes_every_symbol_in_it_test_code() {
        for (qualified_name, file_path, _) in CASES {
            if is_test_file(file_path) {
                assert!(
                    is_test_symbol(qualified_name, file_path),
                    "{qualified_name} in {file_path}"
                );
            }
        }
        assert!(!is_test_file("src/lib.rs"));
        assert!(!is_test_file(""));
        assert!(is_test_file("  tests/integration.rs "));
    }
}
