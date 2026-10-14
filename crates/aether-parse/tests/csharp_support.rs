use std::collections::HashMap;
use std::path::Path;

use aether_core::{EdgeKind, Language, SymbolKind, file_source_id};
use aether_parse::{ExtractedFile, SymbolExtractor, language_for_path};

const BASIC_PATH: &str = "src/Acme.Billing/PaymentService.cs";
const TEST_PATH: &str = "tests/Acme.Billing.Tests/PaymentServiceTests.cs";

fn extract_with_edges(path: &str, source: &str) -> ExtractedFile {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    extractor
        .extract_with_edges_from_source(Language::CSharp, path, source)
        .expect("csharp extraction")
}

fn targets_from(extracted: &ExtractedFile, qualified_name: &str, kind: EdgeKind) -> Vec<String> {
    let source = extracted
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("missing symbol {qualified_name}"));
    let mut targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == kind && edge.source_id == source.id)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn cs_files_map_to_the_csharp_language() {
    assert_eq!(
        language_for_path(Path::new("src/App/Program.cs")),
        Some(Language::CSharp)
    );
    assert_eq!(Language::CSharp.as_str(), "csharp");

    let mut extractor = SymbolExtractor::new().expect("extractor");
    let extracted = extractor
        .extract_with_edges_from_path(
            Path::new(BASIC_PATH),
            include_str!("fixtures/csharp_basic.cs"),
        )
        .expect("extract by path");
    assert!(
        extracted
            .symbols
            .iter()
            .all(|symbol| symbol.language == Language::CSharp)
    );
}

#[test]
fn extracts_csharp_symbols_with_expected_kinds_and_qualified_names() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/csharp_basic.cs"));

    let kinds = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect::<HashMap<_, _>>();
    let expected = [
        ("PaymentService", SymbolKind::Class),
        ("PaymentService::PaymentService", SymbolKind::Method),
        ("PaymentService::Charge", SymbolKind::Method),
        ("PaymentService::Format", SymbolKind::Method),
        ("PaymentService::History", SymbolKind::Method),
        ("PaymentService::Handler", SymbolKind::Class),
        ("PaymentService::Handler::HandleAll", SymbolKind::Method),
        ("PaymentService::Notify", SymbolKind::Method),
        ("PaymentService::Dispose", SymbolKind::Method),
        ("IPaymentService", SymbolKind::Interface),
        ("IPaymentService::CanCharge", SymbolKind::Method),
        ("Money", SymbolKind::Struct),
        ("Money::Equals", SymbolKind::Method),
        ("Receipt", SymbolKind::Class),
        ("Receipt::From", SymbolKind::Method),
        ("Point", SymbolKind::Struct),
        ("Status", SymbolKind::Enum),
        ("ChargedHandler", SymbolKind::TypeAlias),
    ];
    for (qualified_name, kind) in expected {
        assert_eq!(
            kinds.get(qualified_name),
            Some(&kind),
            "unexpected kind for {qualified_name}"
        );
    }
    // Abstract and interface members without a body, properties and fields are part of their
    // type's text; the namespace is left out of qualified names.
    assert_eq!(extracted.symbols.len(), expected.len(), "{kinds:?}");
}

#[test]
fn csharp_symbol_ids_are_stable_with_line_offset_changes() {
    let source = include_str!("fixtures/csharp_basic.cs");
    let ids = |extracted: &ExtractedFile| {
        extracted
            .symbols
            .iter()
            .map(|symbol| (symbol.qualified_name.clone(), symbol.id.clone()))
            .collect::<HashMap<_, _>>()
    };

    let original = ids(&extract_with_edges(BASIC_PATH, source));
    let shifted = ids(&extract_with_edges(BASIC_PATH, &format!("\n\n{source}")));
    assert_eq!(original, shifted);
}

#[test]
fn extracts_csharp_using_edges_from_the_file() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/csharp_basic.cs"));
    let mut usings = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::DependsOn)
        .inspect(|edge| assert_eq!(edge.source_id, file_source_id(BASIC_PATH)))
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    usings.sort();
    assert_eq!(
        usings,
        strings(&[
            "Acme.Data",
            "System",
            "System.Collections.Generic",
            "System.Math",
            "System.Text.Json",
        ])
    );
}

#[test]
fn spells_csharp_call_targets_like_csharp_qualified_names() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/csharp_basic.cs"));

    assert!(
        targets_from(
            &extracted,
            "PaymentService::PaymentService",
            EdgeKind::Calls
        )
        .is_empty()
    );
    // `var` takes its type from `new`; fields, `this.` fields and properties resolve;
    // `base.` goes to the first base type; unqualified calls go to the enclosing class
    // (members of a `using static` class are not resolved); capitalized names are types.
    assert_eq!(
        targets_from(&extracted, "PaymentService::Charge", EdgeKind::Calls),
        strings(&[
            "ILogger::Info",
            "IRepository::Flush",
            "IRepository::Save",
            "Invoice",
            "Invoice::Validate",
            "PaymentService::Format",
            "PaymentService::Max",
            "Receipt::From",
            "ServiceBase::Audit",
        ])
    );
    // A predefined parameter type has no declaration to point at.
    assert_eq!(
        targets_from(&extracted, "PaymentService::Format", EdgeKind::Calls),
        strings(&["ToString"])
    );
    // `foreach` variables and null-conditional calls resolve.
    assert_eq!(
        targets_from(&extracted, "PaymentService::History", EdgeKind::Calls),
        strings(&["IRepository::Find", "Receipt::Print"])
    );
    // Calls in a lambda belong to the enclosing method and its class.
    assert_eq!(
        targets_from(
            &extracted,
            "PaymentService::Handler::HandleAll",
            EdgeKind::Calls
        ),
        strings(&["List::ForEach", "PaymentService::Handler::Handle"])
    );
    // Invoking a delegate parameter has no static target.
    assert_eq!(
        targets_from(&extracted, "PaymentService::Notify", EdgeKind::Calls),
        strings(&["PaymentService::Audit"])
    );
    assert_eq!(
        targets_from(&extracted, "IPaymentService::CanCharge", EdgeKind::Calls),
        strings(&["Customer::IsActive"])
    );
    assert_eq!(
        targets_from(&extracted, "Receipt::From", EdgeKind::Calls),
        strings(&["Receipt"])
    );
}

#[test]
fn extracts_csharp_type_refs_from_signatures() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/csharp_basic.cs"));

    assert_eq!(
        targets_from(
            &extracted,
            "PaymentService::PaymentService",
            EdgeKind::TypeRef
        ),
        strings(&["ILogger", "IRepository"])
    );
    assert_eq!(
        targets_from(&extracted, "PaymentService::Charge", EdgeKind::TypeRef),
        strings(&["Customer", "Receipt"])
    );
    // `IEnumerable` is a base class library type; predefined types never count.
    assert_eq!(
        targets_from(&extracted, "PaymentService::History", EdgeKind::TypeRef),
        strings(&["Customer", "Receipt"])
    );
    assert!(targets_from(&extracted, "PaymentService::Format", EdgeKind::TypeRef).is_empty());
    assert_eq!(
        targets_from(
            &extracted,
            "PaymentService::Handler::HandleAll",
            EdgeKind::TypeRef
        ),
        strings(&["Invoice"])
    );
    assert_eq!(
        targets_from(&extracted, "Receipt::From", EdgeKind::TypeRef),
        strings(&["Invoice", "Receipt"])
    );
}

#[test]
fn extracts_csharp_base_list_as_implements_edges() {
    let extracted = extract_with_edges(BASIC_PATH, include_str!("fixtures/csharp_basic.cs"));

    // `IDisposable` and `IEquatable<T>` come from the base class library.
    assert_eq!(
        targets_from(&extracted, "PaymentService", EdgeKind::Implements),
        strings(&["IPaymentService", "ServiceBase"])
    );
    assert!(targets_from(&extracted, "Money", EdgeKind::Implements).is_empty());
}

#[test]
fn extracts_xunit_nunit_and_mstest_test_intents() {
    let extracted = extract_with_edges(TEST_PATH, include_str!("fixtures/csharp_tests.cs"));

    let intents = extracted
        .test_intents
        .iter()
        .map(|intent| {
            (
                intent.test_name.as_str(),
                intent.intent_text.as_str(),
                intent.group_label.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    let tests = Some("PaymentServiceTests");
    // Nested classes reuse method names, so a nested test is named by its path. The file's
    // helper enum is not a second test class, and a doc comment without a `<summary>` falls
    // back to the method name.
    assert_eq!(
        intents,
        vec![
            ("CafeMenu", "caf\u{e9} menu", tests),
            (
                "Charge_ReturnsReceipt_WhenCustomerIsActive",
                "charge returns receipt when customer is active",
                tests
            ),
            ("DataDriven", "data driven", tests),
            ("Decline", "Declined cards raise an error.", tests),
            ("FromSource", "from source", tests),
            ("Named", "named description", tests),
            ("ParsesNull", "parses \"null\"", tests),
            ("RawText", "raw \"text\" here", tests),
            ("ReadsRaw", "reads \"raw\" input", tests),
            ("Refund", "refunds go back to the card", tests),
            ("Rejects_NegativeAmounts", "rejects negative amounts", tests),
            ("SettlesAsync", "settles async", tests),
            ("TestRounding", "rounding", tests),
            ("Totals", "totals add up", tests),
            (
                "WhenClosed::RejectsLookup",
                "rejects lookup",
                Some("WhenClosed")
            ),
            (
                "WhenEmpty::RejectsLookup",
                "rejects lookup",
                Some("WhenEmpty")
            ),
            ("Zero", "handles zero amounts", tests),
        ]
    );

    for intent in &extracted.test_intents {
        assert_eq!(intent.language, Language::CSharp);
        let symbol = extracted
            .symbols
            .iter()
            .find(|symbol| Some(&symbol.id) == intent.symbol_id.as_ref())
            .expect("intent points at its test method");
        assert_eq!(
            symbol.qualified_name,
            format!("PaymentServiceTests::{}", intent.test_name),
            "{intent:?}"
        );
    }

    // A test method's calls resolve through the test class's fields.
    assert_eq!(
        targets_from(
            &extracted,
            "PaymentServiceTests::Charge_ReturnsReceipt_WhenCustomerIsActive",
            EdgeKind::Calls
        ),
        strings(&["PaymentService::Charge"])
    );
}

#[test]
fn csharp_extraction_tolerates_syntax_errors() {
    let broken = [
        "class Broken : {",
        "class A { void M( { } }",
        "class A { [Fact] void }",
        "using static ;\nclass A {}",
        "class A { void M() { new (); x.; base.(); this?.(); } }",
        "namespace;\nrecord struct",
    ];
    let mut extractor = SymbolExtractor::new().expect("extractor");
    for source in broken {
        extractor
            .extract_with_edges_from_source(Language::CSharp, TEST_PATH, source)
            .unwrap_or_else(|error| panic!("extraction failed for {source:?}: {error}"));
    }
}

const RECEIVERS_PATH: &str = "src/Acme.Billing/Ledger.cs";

#[test]
fn csharp_receivers_resolve_through_every_kind_of_declaration() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/csharp_receivers.cs"));

    // For-loop and pattern variables shadow the fields of the same name (`cursor`, `deposit`);
    // `using` variables and constructor arguments resolve.
    assert_eq!(
        targets_from(&extracted, "Ledger::Post", EdgeKind::Calls),
        strings(&[
            "Account::Cursor",
            "Cursor::Advance",
            "Cursor::HasNext",
            "Cursor::Mark",
            "Deposit::IsLarge",
            "Deposit::Review",
            "Entry",
            "Ledger::Label",
            "Ledger::NextNumber",
            "Ledger::Open",
            "Session::Flush",
        ])
    );
    // Nested types are spelled with their declaring types, written simply or qualified.
    assert_eq!(
        targets_from(&extracted, "Ledger::Create", EdgeKind::Calls),
        strings(&["Ledger::Builder"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::Create", EdgeKind::TypeRef),
        strings(&["Ledger::Builder"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::Configure", EdgeKind::Calls),
        strings(&["Ledger::Builder::Limit", "Ledger::Create"])
    );
    // A nested class calling its outer class's method.
    assert_eq!(
        targets_from(&extracted, "Ledger::Builder::Limit", EdgeKind::Calls),
        strings(&["Ledger::Validate"])
    );
    // Switch statement and switch expression patterns bind their variables.
    assert_eq!(
        targets_from(&extracted, "Ledger::Route", EdgeKind::Calls),
        strings(&["Withdrawal::Cancel"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::Amount", EdgeKind::Calls),
        strings(&["Deposit::Total"])
    );
}

#[test]
fn csharp_receivers_skip_what_has_no_static_target() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/csharp_receivers.cs"));
    let calls = |qualified_name: &str| targets_from(&extracted, qualified_name, EdgeKind::Calls);
    let type_refs =
        |qualified_name: &str| targets_from(&extracted, qualified_name, EdgeKind::TypeRef);

    // A member of a predefined type is not a type name.
    assert_eq!(calls("Ledger::Shout"), strings(&["ToUpper"]));
    // `nameof` is an operator.
    assert_eq!(calls("Ledger::Guard"), strings(&["ArgumentNullException"]));
    // Parameter and tuple element names are not types.
    assert_eq!(type_refs("Ledger::Batch"), strings(&["Entry"]));
    // Raising an event: invoking it directly has no static target.
    assert_eq!(calls("Ledger::Raise"), strings(&["EventHandler::Invoke"]));
    // Type parameters have no declaration to point at: `new T()` gets no edge, and a call on
    // a value typed by one keeps the bare method name.
    assert!(calls("Ledger::Make").is_empty());
    assert!(type_refs("Ledger::Make").is_empty());
    assert_eq!(calls("Ledger::Touch"), strings(&["Validate"]));
    // A delegate field is invoked, not declared.
    assert_eq!(calls("Ledger::Fire"), strings(&["Ledger::Validate"]));
    // Catch variables, `while` patterns and primary-constructor parameters resolve.
    assert_eq!(
        calls("Ledger::Safe"),
        strings(&["Ledger::Open", "LedgerException::Report"])
    );
    assert_eq!(calls("Ledger::Drain"), strings(&["Deposit::Settle"]));
    assert_eq!(calls("Svc::Run"), strings(&["IRepo::Save"]));
    // A namespace-qualified name is not the enclosing class's nested type of the same name,
    // and another class's nested type is not in scope by simple name.
    assert_eq!(calls("Ledger::Render"), strings(&["Node"]));
    assert_eq!(type_refs("Ledger::Render"), strings(&["Node"]));
    assert_eq!(type_refs("Scene::Add"), strings(&["Builder"]));
    // Members of `Result` and `Result<T>` get different ids.
    let to_strings = extracted
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "Result::ToString")
        .collect::<Vec<_>>();
    assert_eq!(to_strings.len(), 2);
    assert_ne!(to_strings[0].id, to_strings[1].id);
}

#[test]
fn csharp_attributes_do_not_change_ids() {
    let source = include_str!("fixtures/csharp_receivers.cs");
    let id_of = |source: &str| {
        extract_with_edges(RECEIVERS_PATH, source)
            .symbols
            .into_iter()
            .find(|symbol| symbol.qualified_name == "Ledger::Reset")
            .map(|symbol| symbol.id)
            .expect("Ledger::Reset")
    };
    let without = source.replace(
        "    [Obsolete]\n    public void Reset()",
        "    public void Reset()",
    );
    let different = source.replace("    [Obsolete]\n", "    [Obsolete(\"use Clear\")]\n");
    assert_ne!(without, source);
    assert_ne!(different, source);
    assert_eq!(id_of(source), id_of(&without));
    assert_eq!(id_of(source), id_of(&different));
}

#[test]
fn csharp_overloads_share_a_qualified_name_but_not_an_id() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/csharp_receivers.cs"));
    let overloads = extracted
        .symbols
        .iter()
        .filter(|symbol| symbol.qualified_name == "Overloads::Find")
        .collect::<Vec<_>>();
    assert_eq!(overloads.len(), 2);
    assert_ne!(overloads[0].id, overloads[1].id);

    // One overload delegating to the other gets no edge: it would resolve to both overloads
    // and read as a cycle between them.
    let mut targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .filter(|edge| overloads.iter().any(|symbol| symbol.id == edge.source_id))
        .map(|edge| edge.target_qualified_name.as_str())
        .collect::<Vec<_>>();
    targets.sort();
    assert_eq!(targets, ["Overloads::Log"]);
}

#[test]
fn csharp_members_inside_preprocessor_blocks_resolve() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/csharp_receivers.cs"));
    let calls = |qualified_name: &str| targets_from(&extracted, qualified_name, EdgeKind::Calls);

    // A property declared under `#if` types its receiver; a method declared under `#if` in the
    // outer class is still the outer class's.
    assert_eq!(calls("Client::Send"), strings(&["ILogger::Info"]));
    assert_eq!(calls("Outer::Inner::Run"), strings(&["Outer::Trace"]));
}

#[test]
fn csharp_dotted_names_keep_types_declared_in_other_files() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/csharp_receivers.cs"));

    // `Payment` and `CreateOrder` are not namespaces the file declares or imports, so the
    // dotted names are their nested types. `Acme.Model.Node` starts at a namespace.
    assert_eq!(
        targets_from(&extracted, "Report::Add", EdgeKind::TypeRef),
        strings(&["Payment::Status"])
    );
    assert_eq!(
        targets_from(&extracted, "Report::Order", EdgeKind::Calls),
        strings(&["CreateOrder::Command", "Send"])
    );
    assert_eq!(
        targets_from(&extracted, "Ledger::Render", EdgeKind::TypeRef),
        strings(&["Node"])
    );
}

#[test]
fn csharp_types_declared_in_the_file_are_not_mistaken_for_framework_types() {
    let extracted =
        extract_with_edges(RECEIVERS_PATH, include_str!("fixtures/csharp_receivers.cs"));

    // The file declares its own `Task`.
    assert_eq!(
        targets_from(&extracted, "Board", EdgeKind::Implements),
        strings(&["Task"])
    );
    assert_eq!(
        targets_from(&extracted, "Board::Add", EdgeKind::TypeRef),
        strings(&["Task"])
    );
}

#[test]
fn csharp_same_named_types_in_two_namespace_blocks_get_different_ids() {
    let ids_of = |source: &str, qualified_name: &str| {
        extract_with_edges(RECEIVERS_PATH, source)
            .symbols
            .into_iter()
            .filter(|symbol| symbol.qualified_name == qualified_name)
            .map(|symbol| symbol.id)
            .collect::<Vec<_>>()
    };
    let two_blocks = "namespace Acme.V1 { public class Mapper { public int Map(int o) => o; } }\n\
                      namespace Acme.V2 { public class Mapper { public int Map(int o) => o; } }\n";
    let block =
        "namespace Acme.V1\n{\n    public class Mapper { public int Map(int o) => o; }\n}\n";
    let file_scoped = "namespace Acme.V1;\n\npublic class Mapper { public int Map(int o) => o; }\n";
    for qualified_name in ["Mapper", "Mapper::Map"] {
        let ids = ids_of(two_blocks, qualified_name);
        assert_eq!(ids.len(), 2, "{qualified_name}");
        assert_ne!(ids[0], ids[1], "{qualified_name}");
        // Converting a namespace block to a file-scoped namespace keeps ids.
        assert_eq!(
            ids_of(block, qualified_name),
            ids_of(file_scoped, qualified_name),
            "{qualified_name}"
        );
        assert_eq!(ids_of(block, qualified_name).len(), 1, "{qualified_name}");
    }
}
