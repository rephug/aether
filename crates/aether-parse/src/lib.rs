mod languages;
mod module_roots;
mod parser;
mod registry;

pub use module_roots::ModuleRoots;
pub use parser::{
    DuplicateSymbolIdentity, ExtractedFile, RustUsePathAtCursor, RustUsePrefix, SymbolExtractor,
    TestIntent, language_for_path, rust_use_path_at_cursor,
};
pub use registry::{LanguageConfig, LanguageHooks, LanguageRegistry};
