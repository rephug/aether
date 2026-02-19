use std::collections::HashMap;
use std::path::Path;

use aether_core::{Language, Symbol, SymbolEdge};
use tree_sitter::{Node, Query, QueryCapture};

use crate::languages;
use crate::parser::TestIntent;

pub struct LanguageConfig {
    pub id: &'static str,
    pub extensions: &'static [&'static str],
    pub ts_language: tree_sitter::Language,
    pub symbol_query: Query,
    pub edge_query: Query,
    pub test_intent_query: Option<Query>,
    pub module_markers: &'static [&'static str],
    pub hooks: Option<Box<dyn LanguageHooks>>,
}

pub trait LanguageHooks: Send + Sync {
    fn qualify_name(
        &self,
        _file_path: &str,
        _symbol_name: &str,
        _parent: Option<&str>,
    ) -> Option<String> {
        None
    }

    fn map_symbol(
        &self,
        _language: Language,
        _captures: &QueryCaptures<'_, '_>,
        _source: &[u8],
        _file_path: &str,
    ) -> Option<Symbol> {
        None
    }

    fn map_edge(
        &self,
        _language: Language,
        _captures: &QueryCaptures<'_, '_>,
        _source: &[u8],
        _file_path: &str,
        _symbols: &[Symbol],
    ) -> Option<Vec<SymbolEdge>> {
        None
    }

    fn map_test_intent(
        &self,
        _language: Language,
        _captures: &QueryCaptures<'_, '_>,
        _source: &[u8],
        _file_path: &str,
        _symbols: &[Symbol],
    ) -> Option<Vec<TestIntent>> {
        None
    }

    fn is_module_root(&self, _dir_path: &Path) -> Option<bool> {
        None
    }
}

pub struct QueryCaptures<'q, 'tree> {
    query: &'q Query,
    captures: &'q [QueryCapture<'tree>],
    pattern_index: usize,
}

impl<'q, 'tree> QueryCaptures<'q, 'tree> {
    pub fn new(
        query: &'q Query,
        captures: &'q [QueryCapture<'tree>],
        pattern_index: usize,
    ) -> Self {
        Self {
            query,
            captures,
            pattern_index,
        }
    }

    pub fn pattern_index(&self) -> usize {
        self.pattern_index
    }

    pub fn first_capture_name_with_prefix(&self, prefix: &str) -> Option<&str> {
        for capture in self.captures {
            let name = self.capture_name(capture.index)?;
            if name.starts_with(prefix) {
                return Some(name);
            }
        }
        None
    }

    pub fn node(&self, name: &str) -> Option<Node<'tree>> {
        self.captures.iter().find_map(|capture| {
            let capture_name = self.capture_name(capture.index)?;
            if capture_name == name {
                Some(capture.node)
            } else {
                None
            }
        })
    }

    pub fn node_with_prefix(&self, prefix: &str) -> Option<Node<'tree>> {
        self.captures.iter().find_map(|capture| {
            let capture_name = self.capture_name(capture.index)?;
            if capture_name.starts_with(prefix) {
                Some(capture.node)
            } else {
                None
            }
        })
    }

    pub fn capture_text(&self, name: &str, source: &[u8]) -> Option<String> {
        let node = self.node(name)?;
        Some(node_text(node, source))
    }

    fn capture_name(&self, index: u32) -> Option<&str> {
        self.query.capture_names().get(index as usize).copied()
    }
}

pub struct LanguageRegistry {
    configs: Vec<LanguageConfig>,
    extension_index: HashMap<String, usize>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        Self {
            configs: Vec::new(),
            extension_index: HashMap::new(),
        }
    }

    pub fn register(&mut self, config: LanguageConfig) {
        let index = self.configs.len();
        for ext in config.extensions {
            let normalized = normalize_extension(ext);
            self.extension_index.entry(normalized).or_insert(index);
        }
        self.configs.push(config);
    }

    pub fn get_by_extension(&self, extension: &str) -> Option<&LanguageConfig> {
        let normalized = normalize_extension(extension);
        let index = *self.extension_index.get(&normalized)?;
        self.configs.get(index)
    }

    pub fn get_by_path(&self, path: &Path) -> Option<&LanguageConfig> {
        let ext = path.extension()?.to_string_lossy();
        self.get_by_extension(&ext)
    }

    pub fn get_by_id(&self, id: &str) -> Option<&LanguageConfig> {
        self.configs.iter().find(|config| config.id == id)
    }

    pub fn configs(&self) -> &[LanguageConfig] {
        &self.configs
    }
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        default_registry()
    }
}

pub fn default_registry() -> LanguageRegistry {
    let mut registry = LanguageRegistry::new();
    registry.register(languages::python::config());
    registry.register(languages::rust::config());
    registry.register(languages::typescript::config());
    registry.register(languages::typescript::tsx_js_config());
    registry
}

fn normalize_extension(extension: &str) -> String {
    extension
        .trim_start_matches('.')
        .trim()
        .to_ascii_lowercase()
}

fn node_text(node: Node<'_>, source: &[u8]) -> String {
    let start = node.start_byte();
    let end = node.end_byte();
    if start >= end || end > source.len() {
        return String::new();
    }
    String::from_utf8_lossy(&source[start..end]).into_owned()
}
