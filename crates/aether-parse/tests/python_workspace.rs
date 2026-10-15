//! Python call targets resolved through the workspace's other files: package re-exports and
//! `src/` layouts seen from outside `src/`.

use std::fs;
use std::path::Path;

use aether_core::EdgeKind;
use aether_parse::SymbolExtractor;
use tempfile::TempDir;

fn workspace(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, source) in files {
        write(dir.path(), path, source);
    }
    dir
}

fn write(root: &Path, path: &str, source: &str) {
    let full_path = root.join(path);
    fs::create_dir_all(full_path.parent().expect("parent")).expect("create dirs");
    fs::write(full_path, source).expect("write file");
}

/// The call targets of `path`, sorted, as an extractor reading `root` spells them.
fn targets(extractor: &mut SymbolExtractor, root: &Path, path: &str) -> Vec<String> {
    let source = fs::read_to_string(root.join(path)).expect("read source");
    let mut targets = extractor
        .extract_with_edges_from_path(Path::new(path), &source)
        .expect("extract")
        .edges
        .into_iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| edge.target_qualified_name)
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

fn sorted(targets: &[&str]) -> Vec<String> {
    let mut targets = targets
        .iter()
        .map(|target| (*target).to_owned())
        .collect::<Vec<_>>();
    targets.sort();
    targets
}

fn extractor(root: &Path) -> SymbolExtractor {
    SymbolExtractor::new()
        .expect("extractor")
        .with_workspace_root(root)
}

const PACKAGE: &[(&str, &str)] = &[
    (
        "pkg/__init__.py",
        r#"from .impl import helper, Widget
from .impl import renamed as public_name
from .api import chained
from .stars import *
from .listed import *


def own():
    return helper()
"#,
    ),
    (
        "pkg/impl.py",
        r#"def helper():
    return 1


def renamed():
    return 2


class Widget:
    def build(self):
        return self
"#,
    ),
    ("pkg/api.py", "from ._core import chained\n"),
    ("pkg/_core.py", "def chained():\n    return 3\n"),
    (
        "pkg/stars.py",
        "def starred():\n    return 4\n\n\ndef _hidden():\n    return 5\n",
    ),
    (
        "pkg/listed.py",
        r#"__all__ = ["exported"]


def exported():
    return 6


def unlisted():
    return 7
"#,
    ),
    (
        "pkg/consumer.py",
        "from . import helper\n\n\ndef use():\n    return helper()\n",
    ),
    (
        "tests/test_pkg.py",
        r#"import pkg
from pkg import helper, Widget, public_name, chained, starred, exported
from pkg import unlisted, own, _hidden, missing


def test_calls():
    helper()
    Widget()
    Widget.build(None)
    public_name()
    chained()
    starred()
    exported()
    unlisted()
    own()
    _hidden()
    missing()
    pkg.helper()
    pkg.Widget.build(None)
"#,
    ),
];

#[test]
fn package_reexports_target_the_defining_module() {
    let dir = workspace(PACKAGE);
    let mut extractor = extractor(dir.path());

    assert_eq!(
        targets(&mut extractor, dir.path(), "tests/test_pkg.py"),
        sorted(&[
            // `from .impl import helper, Widget`, also reached through `import pkg` (the
            // test's edges to one target are one edge).
            "pkg.impl::helper",
            "pkg.impl::Widget",
            "pkg.impl::Widget::build",
            // `from .impl import renamed as public_name`.
            "pkg.impl::renamed",
            // `from .api import chained`, where `api` imports it from `_core`.
            "pkg._core::chained",
            // `from .stars import *` (no `__all__`) and `from .listed import *` (`__all__`).
            "pkg.stars::starred",
            "pkg.listed::exported",
            // Not exported: `listed`'s `__all__` leaves it out.
            "pkg::unlisted",
            // Defined by the package itself.
            "pkg::own",
            // A private name, which `*` doesn't import without `__all__`.
            "pkg::_hidden",
            // Bound nowhere.
            "pkg::missing",
        ])
    );
    // A module of the package importing from it, and the package calling its own import.
    assert_eq!(
        targets(&mut extractor, dir.path(), "pkg/consumer.py"),
        sorted(&["pkg.impl::helper"])
    );
    assert_eq!(
        targets(&mut extractor, dir.path(), "pkg/__init__.py"),
        sorted(&["pkg.impl::helper"])
    );
}

#[test]
fn without_a_workspace_root_targets_keep_the_imported_spelling() {
    let dir = workspace(PACKAGE);
    let mut extractor = SymbolExtractor::new().expect("extractor");
    let targets = targets(&mut extractor, dir.path(), "tests/test_pkg.py");
    assert!(targets.contains(&"pkg::helper".to_owned()), "{targets:?}");
    assert!(targets.contains(&"pkg::starred".to_owned()), "{targets:?}");
}

#[test]
fn ambiguous_reexports_keep_their_target() {
    let dir = workspace(&[
        (
            "amb/__init__.py",
            r#"from .one import *
from .two import *
from .one import shared

try:
    from ._fast import speedy
except ImportError:
    from ._slow import speedy

from .one import both_ways


def both_ways():
    return None
"#,
        ),
        (
            "amb/one.py",
            r#"def dup():
    return 1


def only_one():
    return 2


def shared():
    return 3


def both_ways():
    return 4
"#,
        ),
        ("amb/two.py", "def dup():\n    return 5\n"),
        ("amb/_fast.py", "def speedy():\n    return 6\n"),
        ("amb/_slow.py", "def speedy():\n    return 7\n"),
        (
            "mixed/__init__.py",
            "from os.path import *\nfrom .local import *\n",
        ),
        ("mixed/local.py", "def local_fn():\n    return 8\n"),
        (
            "use_amb.py",
            r#"from amb import dup, only_one, shared, speedy, both_ways
from mixed import local_fn


def run():
    dup()
    only_one()
    shared()
    speedy()
    both_ways()
    local_fn()
"#,
        ),
    ]);
    let mut extractor = extractor(dir.path());
    assert_eq!(
        targets(&mut extractor, dir.path(), "use_amb.py"),
        sorted(&[
            // Both `*` imports bind it.
            "amb::dup",
            "amb.one::only_one",
            // Imported by name and by `*` from the same module.
            "amb.one::shared",
            // Imported from one module or the other.
            "amb::speedy",
            // Imported and defined.
            "amb::both_ways",
            // A `*` import from the standard library may bind any public name.
            "mixed::local_fn",
        ])
    );
}

#[test]
fn src_layout_targets_gain_the_src_prefix() {
    let dir = workspace(&[
        ("src/shop/__init__.py", "from .pricing import quote\n"),
        (
            "src/shop/pricing.py",
            r#"def quote():
    return 1


class Cart:
    def total(self):
        return 2
"#,
        ),
        (
            "src/billing/invoice.py",
            "from shop.pricing import Cart\n\n\ndef bill():\n    return Cart.total(None)\n",
        ),
        (
            "tests/test_shop.py",
            r#"import os
import requests
import shop.pricing
from shop import quote
from shop.pricing import Cart
from helpers import assist


def test_quote():
    quote()
    Cart()
    shop.pricing.quote()
    assist()
    requests.get()
    os.path.join("a", "b")
"#,
        ),
        ("helpers.py", "def assist():\n    return 3\n"),
        ("src/helpers.py", "def assist():\n    return 4\n"),
        (
            "services/api/src/api_pkg/handlers.py",
            "def handle():\n    return 5\n",
        ),
        (
            "services/api/tests/test_handlers.py",
            "from api_pkg.handlers import handle\n\n\ndef test_handle():\n    handle()\n",
        ),
    ]);
    let mut extractor = extractor(dir.path());
    assert_eq!(
        targets(&mut extractor, dir.path(), "tests/test_shop.py"),
        sorted(&[
            // `from shop import quote`, re-exported by `src/shop/__init__.py`, and
            // `shop.pricing.quote()`.
            "src.shop.pricing::quote",
            "src.shop.pricing::Cart",
            // A module both at the workspace root and under `src/` could be either.
            "helpers::assist",
            // Neither a workspace module nor the standard library.
            "requests::get",
            "os.path.join",
        ])
    );
    // A sibling package under `src/`.
    assert_eq!(
        targets(&mut extractor, dir.path(), "src/billing/invoice.py"),
        sorted(&["src.shop.pricing::Cart::total"])
    );
    // The nearest `src/` above the importing file, in a monorepo.
    assert_eq!(
        targets(
            &mut extractor,
            dir.path(),
            "services/api/tests/test_handlers.py"
        ),
        sorted(&["services.api.src.api_pkg.handlers::handle"])
    );
}

/// Cases where Python could bind the name differently from what a naive reading says: the
/// target keeps its spelling rather than name the wrong symbol.
#[test]
fn bindings_python_may_resolve_otherwise_keep_their_target() {
    let dir = workspace(&[
        // A `*` import after an aliased import of the same module rebinds the alias.
        (
            "alias/__init__.py",
            "from .core import load_v2 as load\nfrom .core import *\n",
        ),
        (
            "alias/core.py",
            "def load():\n    return 1\n\n\ndef load_v2():\n    return 2\n",
        ),
        // `__all__` set in either branch: the `*` import may bind `fast_sum`.
        (
            "cond/__init__.py",
            "from .pure import fast_sum\nfrom .accel import *\n",
        ),
        ("cond/pure.py", "def fast_sum(values):\n    return 0\n"),
        (
            "cond/accel.py",
            r#"try:
    from ._speedups import fast_sum
    __all__ = ["fast_sum", "slow_sum"]
except ImportError:
    __all__ = ["slow_sum"]


def slow_sum(values):
    return 0
"#,
        ),
        // `__all__` filled in by a decorator.
        (
            "plug/__init__.py",
            "from .base import render\nfrom .plugins import *\n",
        ),
        ("plug/base.py", "def render():\n    return 1\n"),
        (
            "plug/plugins.py",
            r#"__all__ = []


def export(function):
    __all__.append(function.__name__)
    return function


@export
def render():
    return 2
"#,
        ),
        // A re-exported class whose method the package replaces.
        (
            "patch/__init__.py",
            r#"from .impl import Widget


def _fast(self):
    return self


Widget.build = _fast
"#,
        ),
        (
            "patch/impl.py",
            "class Widget:\n    def build(self):\n        return self\n",
        ),
        // A package's submodule is one of its attributes once imported, so `*` may take it.
        (
            "top/__init__.py",
            "from .one import *\nfrom .two import *\n",
        ),
        ("top/one/__init__.py", "VALUE = 1\n"),
        ("top/one/tool.py", "def run():\n    return 1\n"),
        ("top/two.py", "def tool():\n    return 2\n"),
        (
            "use_cases.py",
            r#"from alias import load
from cond import fast_sum, slow_sum
from plug import render
from patch import Widget
from top import tool


def run():
    load()
    fast_sum([])
    slow_sum([])
    render()
    Widget()
    Widget.build(None)
    tool()
"#,
        ),
        // A script's own directory comes first on `sys.path`.
        ("scripts/helpers.py", "def make():\n    return 1\n"),
        ("src/helpers.py", "def make():\n    return 2\n"),
        (
            "scripts/run.py",
            "from helpers import make\n\n\ndef main():\n    make()\n",
        ),
    ]);
    let mut extractor = extractor(dir.path());
    assert_eq!(
        targets(&mut extractor, dir.path(), "use_cases.py"),
        sorted(&[
            "alias::load",
            "cond::fast_sum",
            "cond.accel::slow_sum",
            "plug::render",
            "patch.impl::Widget",
            "patch::Widget::build",
            "top::tool",
        ])
    );
    assert_eq!(
        targets(&mut extractor, dir.path(), "scripts/run.py"),
        sorted(&["helpers::make"])
    );
}

#[test]
fn edits_that_rebind_no_looked_up_name_leave_readers_alone() {
    let dir = workspace(&[
        ("hub/__init__.py", "from .models import *\n"),
        ("hub/models.py", "def save():\n    return 1\n"),
        (
            "app.py",
            "from hub import save\n\n\ndef run():\n    save()\n",
        ),
    ]);
    let root = dir.path();
    let mut extractor = extractor(root);
    assert_eq!(
        targets(&mut extractor, root, "app.py"),
        sorted(&["hub.models::save"])
    );

    // A new import and a private helper bind names nobody looked up.
    write(
        root,
        "hub/models.py",
        "import logging\n\n\ndef save():\n    return _write()\n\n\ndef _write():\n    return 1\n",
    );
    assert!(extractor.refresh_workspace_file("hub/models.py").is_empty());
    // A new file beside them is no module anyone imported.
    write(root, "hub/README.md", "notes\n");
    assert!(extractor.refresh_workspace_file("hub/README.md").is_empty());

    // Binding `save` differently moves the target.
    write(root, "hub/models.py", "save = None\n");
    assert_eq!(
        extractor.refresh_workspace_file("hub/models.py"),
        vec!["app.py".to_owned()]
    );
    assert_eq!(
        targets(&mut extractor, root, "app.py"),
        sorted(&["hub::save"])
    );

    // A package renamed away.
    fs::rename(root.join("hub"), root.join("hub2")).expect("rename");
    assert_eq!(
        extractor.refresh_workspace_file("hub"),
        vec!["app.py".to_owned()]
    );
    // A reader that is gone reads nothing.
    fs::remove_file(root.join("app.py")).expect("remove");
    assert!(extractor.refresh_workspace_file("app.py").is_empty());
    fs::rename(root.join("hub2"), root.join("hub")).expect("rename back");
    assert!(extractor.refresh_workspace_file("hub").is_empty());
}

#[test]
fn refresh_reports_the_files_whose_targets_changed() {
    let dir = workspace(&[
        ("pkg/__init__.py", "from .old import f\n"),
        ("pkg/old.py", "def f():\n    return 1\n"),
        ("pkg/new.py", "def f():\n    return 2\n"),
        (
            "tests/test_pkg.py",
            r#"from pkg import f
from lib.mod import g


def test_f():
    f()
    g()
"#,
        ),
    ]);
    let root = dir.path();
    let mut extractor = extractor(root);
    assert_eq!(
        targets(&mut extractor, root, "tests/test_pkg.py"),
        sorted(&["pkg.old::f", "lib.mod::g"])
    );
    let stale =
        |extractor: &mut SymbolExtractor, path: &str| extractor.refresh_workspace_file(path);

    // Unchanged, or never read.
    assert!(stale(&mut extractor, "pkg/__init__.py").is_empty());
    assert!(stale(&mut extractor, "pkg/new.py").is_empty());
    assert!(stale(&mut extractor, "tests/test_pkg.py").is_empty());

    // The package re-exports `f` from another module.
    write(root, "pkg/__init__.py", "from .new import f\n");
    assert_eq!(
        stale(&mut extractor, "pkg/__init__.py"),
        vec!["tests/test_pkg.py".to_owned()]
    );
    assert_eq!(
        targets(&mut extractor, root, "tests/test_pkg.py"),
        sorted(&["pkg.new::f", "lib.mod::g"])
    );

    // An edit that binds the same names changes no target.
    write(root, "pkg/new.py", "def f():\n    return 3\n");
    assert!(stale(&mut extractor, "pkg/new.py").is_empty());
    // The module it no longer reads is no longer its concern.
    write(root, "pkg/old.py", "f = None\n");
    assert!(stale(&mut extractor, "pkg/old.py").is_empty());

    // The defining module goes away.
    fs::remove_file(root.join("pkg/new.py")).expect("remove");
    assert_eq!(
        stale(&mut extractor, "pkg/new.py"),
        vec!["tests/test_pkg.py".to_owned()]
    );
    assert_eq!(
        targets(&mut extractor, root, "tests/test_pkg.py"),
        sorted(&["pkg::f", "lib.mod::g"])
    );

    // A module the test imported before it existed appears under `src/`.
    write(root, "src/lib/mod.py", "def g():\n    return 4\n");
    assert_eq!(
        stale(&mut extractor, "src/lib/mod.py"),
        vec!["tests/test_pkg.py".to_owned()]
    );
    assert_eq!(
        targets(&mut extractor, root, "tests/test_pkg.py"),
        sorted(&["pkg::f", "src.lib.mod::g"])
    );
}

#[test]
fn import_cycles_end() {
    let dir = workspace(&[
        ("cyc/__init__.py", "from .a import f\nfrom .b import *\n"),
        ("cyc/a.py", "from cyc import f\n"),
        ("cyc/b.py", "from .c import *\n"),
        ("cyc/c.py", "from .b import *\n"),
        (
            "use_cyc.py",
            "from cyc import f, g\n\n\ndef run():\n    f()\n    g()\n",
        ),
    ]);
    let mut extractor = extractor(dir.path());
    assert_eq!(
        targets(&mut extractor, dir.path(), "use_cyc.py"),
        sorted(&["cyc::f", "cyc::g"])
    );
}
