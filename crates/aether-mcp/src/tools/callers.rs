//! Which call edges really reach a selected symbol.
//!
//! Rust qualified names usually carry no module path (only inline `mod` blocks add
//! one), and the Rust parser records method calls such as `x.alpha()` with the bare
//! method name as their target. A call edge's target can therefore name several
//! symbols: same-named methods of different types, a free function, or the same type
//! declared twice. A caller counts for the selected symbol only when the edge's target
//! narrows to that one symbol or to its overloads. TypeScript records calls by their
//! callee text (`obj.alpha`, `this.alpha`) and Python by the callee's qualified name, so
//! the bare-name expansion applies to Rust only.
//!
//! Overloads share a qualified name, language and file but not an ID: Java and C#
//! overloaded methods (`Type::method`), or a Rust method of a trait a type implements
//! twice (`impl From<u8> for Wrap` and `impl From<u16> for Wrap` both declare
//! `Wrap::from`). A call edge can't tell them apart, so a call to that name counts for
//! every overload. Same-named symbols in different files stay ambiguous unless the
//! caller's file or module (Cargo crate, npm package, Maven or Gradle module, .NET
//! project, ...) narrows them to one, C# partial classes included, since neither Java
//! nor C# puts the package or namespace in the qualified name.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use aether_core::same_language_family;
use aether_parse::ModuleRoots;
use aether_store::{
    NameResolution, NameSite, SqliteStore, SymbolRecord, SymbolRelationStore,
    call_target_candidates, call_targets_of, narrow_call_edge_candidates, narrow_name_candidates,
};

use crate::AetherMcpError;

/// How the recorded calls naming the symbols asked about resolved, counted once per
/// caller and callee across every [`CallerAttribution::callers_of`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CallerCoverage {
    /// Callers with a recorded call whose target names the symbol.
    pub(crate) name_matches: u32,
    /// Those attributed to it.
    pub(crate) attributed: u32,
    /// Of those, the ones [`heuristic_attribution`] matched by a guess.
    pub(crate) heuristic: u32,
    /// Those whose call also fits other symbols the caller's file and module don't rule
    /// out: not attributed, so the symbol may have more callers than listed.
    pub(crate) ambiguous: u32,
    /// Those whose call resolves to another symbol of the same name.
    pub(crate) resolved_elsewhere: u32,
}

pub(crate) struct CallerAttribution<'a> {
    store: &'a SqliteStore,
    coverage: CallerCoverage,
    modules: ModuleRoots,
    /// Whether a Rust bare-name call reaches only methods in the caller's own file
    /// ([`narrow_call_edge_candidates`]), as context views resolve call edges.
    member_calls_within_file: bool,
    candidates_by_target: HashMap<String, Vec<SymbolRecord>>,
    records_by_id: HashMap<String, Option<SymbolRecord>>,
}

impl<'a> CallerAttribution<'a> {
    /// Attribution for `aether_dependencies`: a Rust bare-name call can reach a method
    /// anywhere in the caller's module.
    pub(crate) fn new(store: &'a SqliteStore, workspace_root: &Path) -> Self {
        Self {
            store,
            coverage: CallerCoverage::default(),
            modules: ModuleRoots::new(workspace_root),
            member_calls_within_file: false,
            candidates_by_target: HashMap::new(),
            records_by_id: HashMap::new(),
        }
    }

    /// Attribution for context views, which resolve call edges the same way in both
    /// directions: a Rust bare-name call reaches a method only in the caller's own
    /// file, so calls such as `v.len()` don't crowd a method's callers.
    pub(crate) fn for_context(store: &'a SqliteStore, workspace_root: &Path) -> Self {
        Self {
            member_calls_within_file: true,
            ..Self::new(store, workspace_root)
        }
    }

    /// The callers whose call edges resolve to `symbol` or its overloads alone, ordered
    /// by qualified name and ID.
    ///
    /// An edge counts when its target is the symbol's qualified name or, for a Rust
    /// method, its bare name. The symbols that target could name (in the caller's
    /// language family) are narrowed to the caller's own file; if none are there, to
    /// the caller's module; otherwise all are kept. The caller is attributed only when
    /// every remaining candidate is `symbol` or one of its overloads.
    pub(crate) fn callers_of(
        &mut self,
        symbol: &SymbolRecord,
    ) -> Result<Vec<SymbolRecord>, AetherMcpError> {
        let mut callers = HashMap::<String, SymbolRecord>::new();
        // Callers left out, by why; a caller attributed through another edge is not.
        let mut ambiguous = HashSet::<String>::new();
        let mut elsewhere = HashSet::<String>::new();
        let mut heuristic = 0;
        for target in &call_targets_of(symbol) {
            for edge in SymbolRelationStore::get_callers(self.store, target)? {
                if callers.contains_key(&edge.source_id) {
                    continue;
                }
                let Some(caller) = self.record(edge.source_id.as_str())? else {
                    continue;
                };
                let resolution = self.resolve(&caller, edge.target_qualified_name.as_str())?;
                if resolution.reaches(symbol) {
                    ambiguous.remove(&caller.id);
                    elsewhere.remove(&caller.id);
                    if heuristic_attribution(
                        &self.candidates_by_target[edge.target_qualified_name.as_str()],
                        &caller,
                        edge.target_qualified_name.as_str(),
                        symbol,
                    ) {
                        heuristic += 1;
                    }
                    callers.insert(caller.id.clone(), caller);
                } else if matches!(
                    &resolution,
                    NameResolution::Ambiguous(candidates)
                        if candidates.iter().any(|candidate| candidate.id == symbol.id)
                ) {
                    elsewhere.remove(&caller.id);
                    ambiguous.insert(caller.id);
                } else if !ambiguous.contains(&caller.id) {
                    elsewhere.insert(caller.id);
                }
            }
        }
        let count = |len: usize| u32::try_from(len).unwrap_or(u32::MAX);
        self.coverage.attributed += count(callers.len());
        self.coverage.heuristic += heuristic;
        self.coverage.ambiguous += count(ambiguous.len());
        self.coverage.resolved_elsewhere += count(elsewhere.len());
        self.coverage.name_matches += count(callers.len() + ambiguous.len() + elsewhere.len());

        let mut callers = callers.into_values().collect::<Vec<_>>();
        callers.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(callers)
    }

    /// What [`Self::callers_of`] has seen so far.
    pub(crate) fn coverage(&self) -> CallerCoverage {
        self.coverage
    }

    fn resolve(
        &mut self,
        caller: &SymbolRecord,
        target: &str,
    ) -> Result<NameResolution, AetherMcpError> {
        self.candidates(target)?;
        let candidates = &self.candidates_by_target[target];
        let modules = &mut self.modules;
        let mut module_of = |file_path: &str| modules.module_of(file_path);
        let site = NameSite::of(caller);
        let resolution = if self.member_calls_within_file {
            narrow_call_edge_candidates(candidates, target, site, &mut module_of)
        } else {
            narrow_name_candidates(
                call_target_candidates(candidates, target, caller.language.as_str()),
                Some(site),
                &mut module_of,
            )
        };
        Ok(resolution)
    }

    fn candidates(&mut self, target: &str) -> Result<&Vec<SymbolRecord>, AetherMcpError> {
        if !self.candidates_by_target.contains_key(target) {
            let candidates = self.store.list_symbols_for_call_target(target)?;
            self.candidates_by_target
                .insert(target.to_owned(), candidates);
        }
        Ok(&self.candidates_by_target[target])
    }

    fn record(&mut self, symbol_id: &str) -> Result<Option<SymbolRecord>, AetherMcpError> {
        if let Some(record) = self.records_by_id.get(symbol_id) {
            return Ok(record.clone());
        }
        let record = self.store.get_symbol_record(symbol_id)?;
        self.records_by_id
            .insert(symbol_id.to_owned(), record.clone());
        Ok(record)
    }
}

/// Whether attributing `caller`'s call on `target` to `symbol` rested on a guess rather than
/// an exact name only `symbol` (or its overloads) carries: a Rust call recorded by bare
/// method name, which any type's method of that name could answer, a standard-library one
/// included; or a name other symbols in the caller's language family also carry, which
/// the caller's file or module narrowed to `symbol`.
fn heuristic_attribution(
    candidates: &[SymbolRecord],
    caller: &SymbolRecord,
    target: &str,
    symbol: &SymbolRecord,
) -> bool {
    target != symbol.qualified_name
        || call_target_candidates(candidates, target, caller.language.as_str())
            .iter()
            .filter(|candidate| {
                same_language_family(candidate.language.as_str(), caller.language.as_str())
            })
            .any(|candidate| {
                candidate.qualified_name != symbol.qualified_name
                    || candidate.language != symbol.language
                    || candidate.file_path != symbol.file_path
            })
}
