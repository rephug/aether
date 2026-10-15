use aether_core::{EdgeKind, Language, SymbolKind, file_source_id};
use aether_parse::{ExtractedFile, SymbolExtractor};

const CALLS_PATH: &str = "shop/orders/service.py";

fn extract_with_edges(path: &str, source: &str) -> ExtractedFile {
    let mut extractor = SymbolExtractor::new().expect("extractor");
    extractor
        .extract_with_edges_from_source(Language::Python, path, source)
        .expect("python extraction")
}

/// Every call edge as `(source, target)`, the source spelled as its symbol's qualified name
/// or, for module-level code, as the file's source id.
fn call_edges(extracted: &ExtractedFile) -> Vec<(String, String)> {
    let mut edges = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| {
            let source = extracted
                .symbols
                .iter()
                .find(|symbol| symbol.id == edge.source_id)
                .map_or_else(
                    || edge.source_id.clone(),
                    |symbol| symbol.qualified_name.clone(),
                );
            (source, edge.target_qualified_name.clone())
        })
        .collect::<Vec<_>>();
    edges.sort();
    edges
}

fn calls_from(extracted: &ExtractedFile, source: &str) -> Vec<String> {
    call_edges(extracted)
        .into_iter()
        .filter(|(edge_source, _)| edge_source == source)
        .map(|(_, target)| target)
        .collect()
}

fn pairs(module: &str, expected: &[(&str, &str)]) -> Vec<(String, String)> {
    let spell = |name: &str| {
        name.strip_prefix('~')
            .map_or_else(|| name.to_owned(), |local| format!("{module}::{local}"))
    };
    let mut pairs = expected
        .iter()
        .map(|(source, target)| (spell(source), spell(target)))
        .collect::<Vec<_>>();
    pairs.sort();
    pairs
}

/// Every target in `module` names one of the file's symbols, as the mapper qualifies them.
fn assert_same_module_targets_name_symbols(extracted: &ExtractedFile, module: &str) {
    let prefix = format!("{module}::");
    for (_, target) in call_edges(extracted) {
        if target.starts_with(&prefix) {
            assert!(
                extracted
                    .symbols
                    .iter()
                    .any(|symbol| symbol.qualified_name == target),
                "{target} names no symbol"
            );
        }
    }
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn extracts_python_symbols_with_expected_kinds_and_qualified_names() {
    let source = include_str!("fixtures/python_basic.py");
    let extracted = extract_with_edges("tests/fixtures/python_basic.py", source);

    let names = extracted
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect::<Vec<_>>();

    assert!(names.contains(&"tests.fixtures.python_basic::MODULE_FLAG".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::UserId".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::top_level".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::top_level::nested".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::decorated_function".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker::__init__".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker::factory".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_basic::Worker::run".to_owned()));

    let by_name = extracted
        .symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.kind))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(by_name.get("MODULE_FLAG"), Some(&SymbolKind::Variable));
    assert_eq!(by_name.get("UserId"), Some(&SymbolKind::TypeAlias));
    assert_eq!(by_name.get("top_level"), Some(&SymbolKind::Function));
    assert_eq!(by_name.get("nested"), Some(&SymbolKind::Function));
    assert_eq!(by_name.get("Worker"), Some(&SymbolKind::Class));
    assert_eq!(by_name.get("__init__"), Some(&SymbolKind::Method));
}

#[test]
fn python_symbol_ids_are_stable_with_line_offset_changes() {
    let source = include_str!("fixtures/python_basic.py");
    let shifted = format!("\n\n{source}");

    let original = extract_with_edges("tests/fixtures/python_basic.py", source);
    let shifted = extract_with_edges("tests/fixtures/python_basic.py", &shifted);

    let original_ids = original
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.clone(), symbol.id.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let shifted_ids = shifted
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.clone(), symbol.id.clone()))
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(original_ids, shifted_ids);
}

#[test]
fn extracts_python_calls_and_import_edges() {
    let source = include_str!("fixtures/python_imports.py");
    let extracted = extract_with_edges("tests/fixtures/python_imports.py", source);

    let call_targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    // Imported names are spelled as the symbols they import; `str` is a builtin.
    assert_eq!(
        sorted(call_targets),
        strings(&["core.util::helper", "tests.fixtures.local.thing::format"])
    );

    let depends_targets = extracted
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::DependsOn)
        .map(|edge| edge.target_qualified_name.clone())
        .collect::<Vec<_>>();
    assert!(depends_targets.contains(&"os".to_owned()));
    assert!(depends_targets.contains(&"pkg.alpha".to_owned()));
    assert!(depends_targets.contains(&"pkg.beta".to_owned()));
    assert!(depends_targets.contains(&"core.util.helper".to_owned()));
    assert!(depends_targets.contains(&"tests.fixtures.local.thing".to_owned()));
    assert!(depends_targets.contains(&"tests.shared.model".to_owned()));
    assert!(depends_targets.contains(&"pkg.star".to_owned()));
}

#[test]
fn init_module_uses_package_name_for_qualified_symbols() {
    let source = include_str!("fixtures/python_package/__init__.py");
    let extracted = extract_with_edges("tests/fixtures/python_package/__init__.py", source);

    let names = extracted
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect::<Vec<_>>();
    assert!(names.contains(&"tests.fixtures.python_package::PACKAGE_VALUE".to_owned()));
    assert!(names.contains(&"tests.fixtures.python_package::bootstrap".to_owned()));
}

#[test]
fn spells_python_call_targets_like_python_qualified_names() {
    let extracted = extract_with_edges(CALLS_PATH, include_str!("fixtures/python_calls.py"));
    let module = "shop.orders.service";
    let file = file_source_id(CALLS_PATH);

    // `~name` stands for `shop.orders.service::name`. These are all the call edges in the
    // file; every call not listed has no edge (see the next test for why).
    assert_eq!(
        call_edges(&extracted),
        pairs(
            module,
            &[
                // Names imported with `from … import` (an alias and relative imports
                // included), modules imported with `import … [as …]` and `from . import …`,
                // an imported class's methods, same-file functions (one shadowing the
                // builtin `open`), a same-file class, its constructor and its method.
                ("~Service::place", "shop.orders.models::Order"),
                ("~Service::place", "shop.pricing::quote"),
                ("~Service::place", "shop.billing.tax::apply"),
                ("~Service::place", "shop.billing::charge"),
                ("~Service::place", "shop.inventory.stock::reserve"),
                ("~Service::place", "shop.inventory::Warehouse::default"),
                ("~Service::place", "shop.inventory::Warehouse::locate"),
                ("~Service::place", "shop.orders.events::emit"),
                ("~Service::place", "shop.util::slug"),
                ("~Service::place", "~normalize"),
                ("~Service::place", "~open"),
                ("~Service::place", "~Service::setup"),
                ("~Service::place", "~Service"),
                // The standard library keeps Python's own spelling.
                ("~Service::place", "os.path.join"),
                // `self.` and `cls.` reach the methods of the class that declares the method,
                // not a same-named method of another class.
                ("~Service::__init__", "~Service::setup"),
                ("~Service::create", "~Service::build"),
                ("~Service::build", "~Service::create"),
                // Nested classes are reached through their outer class or `self`.
                ("~Outer::Inner::make", "~Outer::Inner::make"),
                ("~Outer::nested", "~Outer::Inner"),
                // A nested `def`, a nested class and a function-local import.
                ("~process", "~process::inner"),
                ("~process", "shop.orders.helpers::late"),
                ("~factory", "~factory::Local"),
                // A default value is evaluated outside the function, where `normalize` is the
                // module's.
                ("~configured", "~normalize"),
                // `global` and `nonlocal` send the lookup to the module or the enclosing
                // function.
                ("~reads_global", "~normalize"),
                ("~tally", "~tally::run"),
                ("~tally::run", "~tally::step"),
                ("~counter", "~counter::run"),
                // A class-body helper used as a decorator is not a method; the methods it
                // decorates are.
                ("~Registry::run", "~Registry::setup"),
                // An instance attribute shadows only the method it is named after, and a
                // function-local class is still a class.
                ("~Button::press", "~Button::render"),
                ("~make_noop_handler", "~make_noop_handler::Handler"),
                // Recursion, also from a comprehension's first iterable, which is evaluated
                // outside the comprehension.
                ("~recurse", "~recurse"),
                ("~items", "~items"),
                // A class pattern names its class; it doesn't capture it.
                ("~matches", "~Service"),
                // Function-local imports of the standard library.
                ("~stdlib_calls", "json.dumps"),
                ("~stdlib_calls", "os.path.join"),
                ("~stdlib_calls", "collections.OrderedDict"),
                ("~stdlib_calls", "collections.OrderedDict.fromkeys"),
                // Module-level and class-body code is attributed to the file. It sees the
                // class body's own names, including from a comprehension's first iterable,
                // and only the names bound before it runs: a loop's target is bound after its
                // iterable is evaluated, and `later` is defined after the `__main__` block
                // calls it.
                (file.as_str(), "~Outer::build_default"),
                (file.as_str(), "~Batches::source"),
                (file.as_str(), "~process"),
            ]
        )
    );
    assert_same_module_targets_name_symbols(&extracted, module);
}

#[test]
fn python_calls_without_a_static_target_have_no_edge() {
    let extracted = extract_with_edges(CALLS_PATH, include_str!("fixtures/python_calls.py"));
    let module = "shop.orders.service";

    for (source, why) in [
        (
            "Service::unresolved",
            "methods on attributes, on methods the class doesn't declare, on string literals, \
             on call results and on super(); builtins; names from a * import; calls of \
             expressions that aren't names; an import past the top-level package; a def that \
             stands in for a failed import",
        ),
        (
            "Service::helper",
            "a @staticmethod's first parameter is not an instance",
        ),
        (
            "Service::clean",
            "neither is the first parameter of a method rebound by staticmethod()",
        ),
        (
            "Service::keyword_only",
            "nor a keyword-only first parameter",
        ),
        (
            "Special::go",
            "only the declaring class's own methods count",
        ),
        (
            "Outer::rebuild",
            "a method can't see its class body's names",
        ),
        (
            "Local::go",
            "methods of a class declared in a function share names with other classes'",
        ),
        (
            "shadows",
            "parameters, loop, except, with, walrus, match and comprehension targets, plain, \
             annotated, augmented and chained assignments, type aliases, del and lambda \
             parameters all shadow the module-level names they share",
        ),
        (
            "install",
            "a function that rebinds a module-level def through `global` leaves it ambiguous",
        ),
        ("uses_patched", "for every caller"),
        (
            "counter::reset",
            "likewise through `nonlocal`, rebinding the enclosing function's def",
        ),
        ("counter::run", "for every caller"),
        (
            "builder",
            "a class body that binds a name later looks it up in the module, skipping the \
             enclosing function",
        ),
        (
            "Registry::_register",
            "a class-body helper the class uses while it runs is not a method",
        ),
        (
            "calls_later",
            "a walrus in a default value rebinds the name in the enclosing scope",
        ),
        (
            "Handler::run",
            "a method's name is shared with a method of a class declared in a function, \
             which the symbol mapper names the same",
        ),
        (
            "Toggle::flip",
            "a method the module replaces with `Class.name = …` may no longer be the def",
        ),
        (
            "walrus_comment",
            "a walrus binds its target even with a comment before `:=`",
        ),
        ("walrus_continuation", "or a line continuation"),
    ] {
        assert!(
            calls_from(&extracted, &format!("{module}::{source}")).is_empty(),
            "{source}: {why}"
        );
    }

    // A nested class body can't see its outer class body's names, so `RESULT =
    // build_default()` in `Outer.Inner` has no edge: the file's only edges are `DEFAULTS` and
    // `NAMES` in `Outer`, the loop in `Batches` and the `__main__` block.
    assert_eq!(
        calls_from(&extracted, &file_source_id(CALLS_PATH)),
        strings(&[
            "shop.orders.service::Batches::source",
            "shop.orders.service::Outer::build_default",
            "shop.orders.service::process",
        ])
    );

    // No target is raw expression text or a bare method name: each one is qualified, or a
    // standard-library name.
    for edge in &extracted.edges {
        let target = edge.target_qualified_name.as_str();
        assert!(
            !target.contains(['(', ')', '[', '"', '\'', ' ']),
            "raw expression text stored as a target: {target}"
        );
        if edge.edge_kind == EdgeKind::Calls {
            assert!(
                target.contains("::")
                    || ["os.", "json.", "collections."]
                        .iter()
                        .any(|stdlib| target.starts_with(stdlib)),
                "a call target that names no module: {target}"
            );
        }
    }
}

#[test]
fn python_call_targets_follow_src_layout_module_paths() {
    // Modules under `src/` are qualified with the `src` prefix, so absolute imports of the
    // file's own top-level package gain it too, from the innermost `src` directory.
    // Standard-library imports keep their path.
    for (path, prefix) in [
        ("src/shop/orders/service.py", "src."),
        (
            "services/api/src/shop/orders/service.py",
            "services.api.src.",
        ),
    ] {
        let extracted = extract_with_edges(path, include_str!("fixtures/python_calls.py"));
        let module = format!("{prefix}shop.orders.service");
        assert_eq!(
            calls_from(&extracted, &format!("{module}::Service::place")),
            sorted(
                [
                    "shop.billing.tax::apply",
                    "shop.billing::charge",
                    "shop.inventory.stock::reserve",
                    "shop.inventory::Warehouse::default",
                    "shop.inventory::Warehouse::locate",
                    "shop.orders.events::emit",
                    "shop.orders.models::Order",
                    "shop.orders.service::Service",
                    "shop.orders.service::Service::setup",
                    "shop.orders.service::normalize",
                    "shop.orders.service::open",
                    "shop.pricing::quote",
                    "shop.util::slug",
                ]
                .iter()
                .map(|target| format!("{prefix}{target}"))
                .chain(["os.path.join".to_owned()])
                .collect()
            ),
            "{path}"
        );
        assert_same_module_targets_name_symbols(&extracted, &module);
    }
}

#[test]
fn python_package_init_calls_resolve_against_the_package() {
    let source = r#"
from .service import Service
from . import events
from .. import beyond


def make():
    return Service(None)


def notify():
    events.emit("made")
    beyond.go()
    return make()
"#;

    // `__init__.py` is the package's own module, so relative imports start from the package.
    // `..` climbs past the top-level package and names nothing.
    let extracted = extract_with_edges("shop/__init__.py", source);
    assert_eq!(
        call_edges(&extracted),
        pairs(
            "shop",
            &[
                ("~make", "shop.service::Service"),
                ("~notify", "shop.events::emit"),
                ("~notify", "~make"),
            ]
        )
    );
    assert_same_module_targets_name_symbols(&extracted, "shop");

    // A top-level `__init__.py` is the workspace root's package, whose modules are named
    // without a prefix.
    let extracted = extract_with_edges("__init__.py", source);
    assert_eq!(
        calls_from(&extracted, "module::notify"),
        strings(&["events::emit", "module::make"])
    );
    assert_eq!(
        calls_from(&extracted, "module::make"),
        strings(&["service::Service"])
    );

    // A top-level module is in no package, so its relative imports name nothing.
    let extracted = extract_with_edges("script.py", source);
    assert_eq!(
        call_edges(&extracted),
        pairs("script", &[("~notify", "~make")])
    );
}

#[test]
fn python_calls_reach_decorated_definitions() {
    let source = include_str!("fixtures/python_basic.py");
    let extracted = extract_with_edges("tests/fixtures/python_basic.py", source);

    // Decorated functions and classes are targets like any other; `sum` is a builtin and
    // `name.upper()` a method on a parameter.
    assert_eq!(
        call_edges(&extracted),
        pairs(
            "tests.fixtures.python_basic",
            &[
                ("~Worker::factory", "~Worker"),
                ("~Worker::run", "~decorated_function"),
                ("~top_level", "~top_level::nested"),
            ]
        )
    );
}

#[test]
fn python_imports_the_file_cannot_spell_store_no_raw_text() {
    // A top-level script's relative import names no module; it used to be stored as its
    // raw text.
    let extracted = extract_with_edges("script.py", "from . import *\n");
    assert!(extracted.edges.is_empty(), "{:?}", extracted.edges);
}

#[test]
fn python_workspace_packages_named_like_the_standard_library_keep_qualified_targets() {
    let source = "from email.templates import render\nimport json\n\n\ndef send():\n    render()\n    json.dumps({})\n";

    // Inside a workspace package called `email`, `email.templates` is that package's module;
    // `json` is still the standard library's.
    let extracted = extract_with_edges("email/sender.py", source);
    assert_eq!(
        calls_from(&extracted, "email.sender::send"),
        strings(&["email.templates::render", "json.dumps"])
    );
}
