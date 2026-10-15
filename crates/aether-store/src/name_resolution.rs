//! Which symbol a qualified name written in a source file names.
//!
//! Qualified names are not unique: Rust, Java and C# leave the module, package or
//! namespace out, so `Config::load` can be declared by several crates, modules or
//! projects, and different languages can declare the same name. Lookups by name narrow
//! the candidates the way a reader of the referring file would, and say so when that
//! leaves more than one instead of picking one.

use aether_core::same_language_family;

use super::*;

/// Where a qualified name was written: the file and language of the referring symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameSite<'a> {
    pub file_path: &'a str,
    pub language: &'a str,
}

impl<'a> NameSite<'a> {
    pub fn of(symbol: &'a SymbolRecord) -> Self {
        Self {
            file_path: symbol.file_path.as_str(),
            language: symbol.language.as_str(),
        }
    }
}

/// What a qualified name resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameResolution {
    /// No candidate symbol.
    NotFound,
    /// One symbol, or a symbol and its overloads (same qualified name, language and
    /// file, which a name can't tell apart), ordered by ID.
    Found(Vec<SymbolRecord>),
    /// Candidates in several files, or different symbols in one file, that the
    /// referring file and its module don't narrow to one, ordered by file and ID.
    Ambiguous(Vec<SymbolRecord>),
}

impl NameResolution {
    /// The symbol the name resolves to: the lowest-ID overload when it has several.
    pub fn found(&self) -> Option<&SymbolRecord> {
        match self {
            Self::Found(symbols) => symbols.first(),
            Self::NotFound | Self::Ambiguous(_) => None,
        }
    }

    /// Whether the name resolves to `symbol` or one of its overloads.
    pub fn reaches(&self, symbol: &SymbolRecord) -> bool {
        matches!(self, Self::Found(symbols) if symbols.iter().any(|found| found.id == symbol.id))
    }
}

/// What context output shows in place of the file of a dependency whose name is
/// ambiguous.
pub fn ambiguity_label(candidates: &[SymbolRecord]) -> String {
    format!("ambiguous: {} candidates", candidates.len())
}

/// Most files [`ambiguity_note`] names.
const AMBIGUITY_NOTE_FILES: usize = 5;

/// Says which files declare the candidates of an ambiguous name (ordered by file, as
/// [`NameResolution::Ambiguous`] has them), for context output.
pub fn ambiguity_note(candidates: &[SymbolRecord]) -> String {
    let mut files = candidates
        .iter()
        .map(|candidate| candidate.file_path.as_str())
        .collect::<Vec<_>>();
    files.dedup();
    let more = files.len().saturating_sub(AMBIGUITY_NOTE_FILES);
    files.truncate(AMBIGUITY_NOTE_FILES);
    let more = match more {
        0 => String::new(),
        1 => " and 1 more file".to_owned(),
        more => format!(" and {more} more files"),
    };
    format!(
        "Ambiguous: {} symbols share this name ({}{more}).",
        candidates.len(),
        files.join(", ")
    )
}

/// Narrows the symbols a qualified name can name, as written at `site`.
///
/// With a site, only candidates in the site's language family count, and they are
/// narrowed to the site's own file; if none are there, to the site's module
/// (`module_of` gives a file's module, `None` when unknown); otherwise all are kept.
/// Without a site, all candidates are kept. The name resolves when what remains is one
/// symbol or a symbol and its overloads.
pub fn narrow_name_candidates(
    candidates: Vec<SymbolRecord>,
    site: Option<NameSite<'_>>,
    module_of: &mut dyn FnMut(&str) -> Option<PathBuf>,
) -> NameResolution {
    let mut candidates = match site {
        Some(site) => candidates
            .into_iter()
            .filter(|candidate| same_language_family(candidate.language.as_str(), site.language))
            .collect::<Vec<_>>(),
        None => candidates,
    };
    if let Some(site) = site {
        if candidates
            .iter()
            .any(|candidate| candidate.file_path == site.file_path)
        {
            candidates.retain(|candidate| candidate.file_path == site.file_path);
        } else if let Some(module) = module_of(site.file_path) {
            let in_module = candidates
                .iter()
                .filter(|candidate| {
                    module_of(candidate.file_path.as_str()).as_ref() == Some(&module)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !in_module.is_empty() {
                candidates = in_module;
            }
        }
    }

    candidates.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then_with(|| left.id.cmp(&right.id))
    });
    candidates.dedup_by(|left, right| left.id == right.id);
    match candidates.first() {
        None => NameResolution::NotFound,
        Some(first)
            if candidates.iter().all(|candidate| {
                candidate.qualified_name == first.qualified_name
                    && candidate.language == first.language
                    && candidate.file_path == first.file_path
            }) =>
        {
            NameResolution::Found(candidates)
        }
        Some(_) => NameResolution::Ambiguous(candidates),
    }
}

/// The targets a call to `symbol` can be recorded under: its qualified name and, for a
/// Rust method or function with a qualified name, its bare name, since the Rust parser
/// records a method call such as `x.alpha()` by the method name alone.
pub fn call_targets_of(symbol: &SymbolRecord) -> Vec<String> {
    let mut targets = vec![symbol.qualified_name.clone()];
    if symbol.language == "rust"
        && matches!(symbol.kind.as_str(), "method" | "function")
        && let Some((_, leaf)) = symbol.qualified_name.rsplit_once("::")
        && !leaf.is_empty()
    {
        targets.push(leaf.to_owned());
    }
    targets
}

/// The symbols a call target written in `site_language` can name, out of `candidates`
/// from [`SqliteStore::list_symbols_for_call_target`]: in Rust a bare target also names
/// each `Type::<target>`; elsewhere a target names exactly the symbols with that
/// qualified name.
pub fn call_target_candidates(
    candidates: &[SymbolRecord],
    target: &str,
    site_language: &str,
) -> Vec<SymbolRecord> {
    let bare_member_calls = site_language == "rust";
    candidates
        .iter()
        .filter(|candidate| bare_member_calls || candidate.qualified_name == target)
        .cloned()
        .collect()
}

/// Narrows the symbols a call edge's target can name when `site` writes it, for context
/// views. The Rust parser records a method call by the method name alone
/// (`self.alpha()` → `alpha`), so for a Rust site a bare target names the symbols with
/// that name declared in the site's own file, `Type::alpha` included, when there are
/// any. Otherwise, and in other languages, it names exactly the symbols with that
/// qualified name. Unlike [`call_target_candidates`], which `aether_dependencies` uses,
/// a bare target never reaches a method in another file, so a call such as `v.len()`
/// on a library type doesn't count for a workspace `MyBuf::len`.
///
/// `candidates` come from [`SqliteStore::list_symbols_for_call_target`].
pub fn narrow_call_edge_candidates(
    candidates: &[SymbolRecord],
    target: &str,
    site: NameSite<'_>,
    module_of: &mut dyn FnMut(&str) -> Option<PathBuf>,
) -> NameResolution {
    let target = target.trim();
    if site.language == "rust" && !target.contains("::") {
        let in_file = candidates
            .iter()
            .filter(|candidate| {
                candidate.file_path == site.file_path
                    && same_language_family(candidate.language.as_str(), site.language)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !in_file.is_empty() {
            return narrow_name_candidates(in_file, Some(site), module_of);
        }
    }
    let exact = candidates
        .iter()
        .filter(|candidate| candidate.qualified_name == target)
        .cloned()
        .collect();
    narrow_name_candidates(exact, Some(site), module_of)
}

impl SqliteStore {
    /// Every symbol with exactly this qualified name, ordered by file and ID.
    pub fn list_symbols_by_qualified_name(
        &self,
        qualified_name: &str,
    ) -> Result<Vec<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE qualified_name = ?1
            ORDER BY file_path ASC, id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![qualified_name], |row| {
            Ok(SymbolRecord {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                qualified_name: row.get(4)?,
                signature_fingerprint: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Resolves the target of a call edge written at `site`, for context views (see
    /// [`narrow_call_edge_candidates`]).
    pub fn resolve_call_edge(
        &self,
        target: &str,
        site: NameSite<'_>,
        module_of: &mut dyn FnMut(&str) -> Option<PathBuf>,
    ) -> Result<NameResolution, StoreError> {
        let candidates = self.list_symbols_for_call_target(target)?;
        Ok(narrow_call_edge_candidates(
            &candidates,
            target,
            site,
            module_of,
        ))
    }

    /// Resolves `qualified_name` as written at `site` (see [`narrow_name_candidates`]).
    pub fn resolve_qualified_name(
        &self,
        qualified_name: &str,
        site: Option<NameSite<'_>>,
        module_of: &mut dyn FnMut(&str) -> Option<PathBuf>,
    ) -> Result<NameResolution, StoreError> {
        let candidates = self.list_symbols_by_qualified_name(qualified_name)?;
        Ok(narrow_name_candidates(candidates, site, module_of))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(id: &str, file_path: &str, language: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: language.to_owned(),
            kind: "method".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 0,
        }
    }

    /// Modules are the first path segment.
    fn first_segment(file_path: &str) -> Option<PathBuf> {
        file_path.split('/').next().map(PathBuf::from)
    }

    fn ids(symbols: &[SymbolRecord]) -> Vec<&str> {
        symbols.iter().map(|symbol| symbol.id.as_str()).collect()
    }

    #[test]
    fn narrowing_prefers_the_referring_file_then_its_module() {
        let candidates = vec![
            symbol("b-load", "billing/Config.java", "java", "Config::load"),
            symbol("o-load", "orders/Config.java", "java", "Config::load"),
            symbol("ts-load", "web/config.ts", "typescript", "Config::load"),
        ];
        let at = |file_path| {
            Some(NameSite {
                file_path,
                language: "java",
            })
        };

        let in_file = narrow_name_candidates(
            candidates.clone(),
            at("billing/Config.java"),
            &mut first_segment,
        );
        assert_eq!(in_file, NameResolution::Found(vec![candidates[0].clone()]));

        let in_module = narrow_name_candidates(
            candidates.clone(),
            at("orders/Service.java"),
            &mut first_segment,
        );
        assert!(in_module.reaches(&candidates[1]));
        assert!(!in_module.reaches(&candidates[0]));

        let elsewhere =
            narrow_name_candidates(candidates.clone(), at("app/Main.java"), &mut first_segment);
        match elsewhere {
            NameResolution::Ambiguous(found) => assert_eq!(ids(&found), ["b-load", "o-load"]),
            other => panic!("expected ambiguity, got {other:?}"),
        }

        // A JavaScript file reaches the TypeScript declaration; a Python file reaches none.
        let from_js = narrow_name_candidates(
            candidates.clone(),
            Some(NameSite {
                file_path: "app/main.js",
                language: "javascript",
            }),
            &mut first_segment,
        );
        assert_eq!(
            from_js.found().map(|found| found.id.as_str()),
            Some("ts-load")
        );
        let from_python = narrow_name_candidates(
            candidates.clone(),
            Some(NameSite {
                file_path: "billing/config.py",
                language: "python",
            }),
            &mut first_segment,
        );
        assert_eq!(from_python, NameResolution::NotFound);
    }

    #[test]
    fn overloads_resolve_together_and_selectors_without_a_site_keep_every_candidate() {
        let overloads = vec![
            symbol("get-2", "billing/Users.java", "java", "Users::get"),
            symbol("get-1", "billing/Users.java", "java", "Users::get"),
        ];
        let resolved = narrow_name_candidates(overloads.clone(), None, &mut first_segment);
        assert_eq!(
            resolved,
            NameResolution::Found(vec![overloads[1].clone(), overloads[0].clone()])
        );
        assert_eq!(
            resolved.found().map(|found| found.id.as_str()),
            Some("get-1")
        );
        assert!(resolved.reaches(&overloads[0]));

        let mut two_files = overloads.clone();
        two_files.push(symbol("get-3", "orders/Users.java", "java", "Users::get"));
        assert!(matches!(
            narrow_name_candidates(two_files, None, &mut first_segment),
            NameResolution::Ambiguous(found) if found.len() == 3
        ));
        assert_eq!(
            narrow_name_candidates(Vec::new(), None, &mut first_segment),
            NameResolution::NotFound
        );
    }

    #[test]
    fn rust_bare_call_targets_reach_methods_in_the_callers_file_only() {
        let rust = |id: &str, file_path: &str, qualified_name: &str| {
            symbol(id, file_path, "rust", qualified_name)
        };
        // `list_symbols_for_call_target("helper")`: the exact name and every `::helper`.
        let helpers = vec![
            rust("s-helper", "core/src/lib.rs", "S::helper"),
            rust("free-helper", "core/src/util.rs", "helper"),
        ];
        let at = |file_path, language| NameSite {
            file_path,
            language,
        };

        // `self.helper()` inside `S`'s file reaches `S::helper`, not the free function.
        let in_impl = narrow_call_edge_candidates(
            &helpers,
            "helper",
            at("core/src/lib.rs", "rust"),
            &mut first_segment,
        );
        assert_eq!(
            in_impl.found().map(|found| found.id.as_str()),
            Some("s-helper")
        );
        // Elsewhere a bare `helper()` names the free function only.
        let elsewhere = narrow_call_edge_candidates(
            &helpers,
            "helper",
            at("core/src/main.rs", "rust"),
            &mut first_segment,
        );
        assert_eq!(
            elsewhere.found().map(|found| found.id.as_str()),
            Some("free-helper")
        );

        // `v.len()` outside `MyBuf`'s file doesn't count for `MyBuf::len`.
        let lens = vec![rust("buf-len", "core/src/buf.rs", "MyBuf::len")];
        assert_eq!(
            narrow_call_edge_candidates(
                &lens,
                "len",
                at("core/src/main.rs", "rust"),
                &mut first_segment
            ),
            NameResolution::NotFound
        );
        // Other languages record calls by qualified name, so only exact names count.
        let java = vec![
            symbol("get", "billing/Users.java", "java", "Users::get"),
            symbol("other", "billing/Users.java", "java", "Other::get"),
        ];
        assert_eq!(
            narrow_call_edge_candidates(
                &java,
                "Users::get",
                at("billing/App.java", "java"),
                &mut first_segment
            )
            .found()
            .map(|found| found.id.as_str()),
            Some("get")
        );
    }

    #[test]
    fn ambiguity_notes_name_at_most_five_files() {
        let candidates = (0..7)
            .map(|index| {
                symbol(
                    &format!("s{index}"),
                    &format!("m{index}/A.java"),
                    "java",
                    "A::f",
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ambiguity_note(&candidates[..6]),
            "Ambiguous: 6 symbols share this name (m0/A.java, m1/A.java, m2/A.java, m3/A.java, \
             m4/A.java and 1 more file)."
        );
        assert_eq!(
            ambiguity_note(&candidates[..2]),
            "Ambiguous: 2 symbols share this name (m0/A.java, m1/A.java)."
        );
        assert_eq!(
            ambiguity_note(&candidates),
            "Ambiguous: 7 symbols share this name (m0/A.java, m1/A.java, m2/A.java, m3/A.java, \
             m4/A.java and 2 more files)."
        );
    }
}
