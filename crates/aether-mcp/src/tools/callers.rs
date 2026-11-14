//! Which stored call edges reach a selected symbol.
//!
//! Rust and ECMAScript references require parser-recorded target identity or file
//! provenance. Name-only references remain visible in the raw store but cannot
//! become callers through a same-name lookup. Other languages retain their
//! existing same-file/module narrowing and overload behavior.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use aether_core::{SymbolEdge, same_language_family};
use aether_parse::ModuleRoots;
use aether_store::{
    NameResolution, NameSite, SqliteStore, SymbolRecord, SymbolRelationStore,
    call_target_candidates, call_targets_of, narrow_symbol_edge_candidates,
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
    candidates_by_target: HashMap<String, Vec<SymbolRecord>>,
    records_by_id: HashMap<String, Option<SymbolRecord>>,
}

impl<'a> CallerAttribution<'a> {
    /// Attribution for `aether_dependencies`, constrained by each edge's proof.
    pub(crate) fn new(store: &'a SqliteStore, workspace_root: &Path) -> Self {
        Self {
            store,
            coverage: CallerCoverage::default(),
            modules: ModuleRoots::new(workspace_root),
            candidates_by_target: HashMap::new(),
            records_by_id: HashMap::new(),
        }
    }

    /// Context views use the same provenance rules in both graph directions.
    pub(crate) fn for_context(store: &'a SqliteStore, workspace_root: &Path) -> Self {
        Self::new(store, workspace_root)
    }

    /// The callers whose call edges resolve to `symbol` or its overloads alone, ordered
    /// by qualified name and ID.
    ///
    /// Cached candidates collect names; the individual edge's provenance determines
    /// attribution. Legacy languages can still narrow an unproved name by module,
    /// and retain overload groups when no unique declaration can be distinguished.
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
                let resolution = self.resolve(&caller, &edge)?;
                if resolution.reaches(symbol) {
                    ambiguous.remove(&caller.id);
                    elsewhere.remove(&caller.id);
                    if edge.target_symbol_id.is_none()
                        && edge.target_file_path.is_none()
                        && heuristic_attribution(
                            &self.candidates_by_target[edge.target_qualified_name.as_str()],
                            &caller,
                            edge.target_qualified_name.as_str(),
                            symbol,
                        )
                    {
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
        edge: &SymbolEdge,
    ) -> Result<NameResolution, AetherMcpError> {
        let target = edge.target_qualified_name.as_str();
        self.candidates(target)?;
        let candidates = &self.candidates_by_target[target];
        let modules = &mut self.modules;
        let mut module_of = |file_path: &str| modules.module_of(file_path);
        let resolution =
            narrow_symbol_edge_candidates(candidates, edge, NameSite::of(caller), &mut module_of);
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
