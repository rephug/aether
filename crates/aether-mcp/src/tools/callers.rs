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

use std::collections::HashMap;
use std::path::Path;

use aether_parse::ModuleRoots;
use aether_store::{
    NameSite, SqliteStore, SymbolRecord, SymbolRelationStore, call_target_candidates,
    call_targets_of, narrow_call_edge_candidates, narrow_name_candidates,
};

use crate::AetherMcpError;

pub(crate) struct CallerAttribution<'a> {
    store: &'a SqliteStore,
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
        for target in &call_targets_of(symbol) {
            for edge in SymbolRelationStore::get_callers(self.store, target)? {
                if callers.contains_key(&edge.source_id) {
                    continue;
                }
                let Some(caller) = self.record(edge.source_id.as_str())? else {
                    continue;
                };
                if self.resolves_to(&caller, edge.target_qualified_name.as_str(), symbol)? {
                    callers.insert(caller.id.clone(), caller);
                }
            }
        }

        let mut callers = callers.into_values().collect::<Vec<_>>();
        callers.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(callers)
    }

    fn resolves_to(
        &mut self,
        caller: &SymbolRecord,
        target: &str,
        symbol: &SymbolRecord,
    ) -> Result<bool, AetherMcpError> {
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
        Ok(resolution.reaches(symbol))
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
