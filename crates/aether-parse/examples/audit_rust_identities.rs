//! Read-only audit: cargo run -p aether-parse --example audit_rust_identities -- ROOT
//! Never opens an index, follows symlinks or calls a provider. Reports the first
//! extraction error per file; it cannot count every colliding definition in one file.
use std::fs;
use std::path::{Path, PathBuf};

use aether_core::Language;
use aether_parse::SymbolExtractor;
use anyhow::{Context, Result};

const MAX_FILES: usize = 10_000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_BYTES: usize = 64 * 1024 * 1024;

fn quoted(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn walk(path: &Path, files: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) => {
                errors.push(format!("{}: {error}", entry.path().display()));
                continue;
            }
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if !matches!(
                entry.file_name().to_str(),
                Some(
                    ".git" | ".aether" | ".codex" | "target" | "node_modules" | "vendor" | ".venv"
                )
            ) {
                walk(&entry.path(), files, errors);
            }
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "rs")
        {
            files.push(entry.path());
        }
    }
}

fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args_os().nth(1).unwrap_or_else(|| ".".into()))
        .canonicalize()
        .context("resolve identity-audit root")?;
    let mut files = Vec::new();
    let mut read_errors = Vec::new();
    walk(&root, &mut files, &mut read_errors);
    files.sort();
    let mut extractor = SymbolExtractor::new().context("initialize read-only Rust parser")?;
    let mut hash = blake3::Hasher::new();
    let mut blocked = Vec::new();
    let mut skipped = 0;
    let mut read = 0;
    let mut parsed = 0;
    let mut symbols = 0;
    let mut bytes = 0;
    for path in files {
        if read >= MAX_FILES || bytes >= MAX_BYTES {
            skipped += 1;
            continue;
        }
        let relative = path
            .strip_prefix(&root)
            .context("audit path escaped root")?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                read_errors.push(format!("{relative}: {error}"));
                continue;
            }
        };
        if metadata.len() > MAX_FILE_BYTES || metadata.len() as usize > MAX_BYTES - bytes {
            skipped += 1;
            continue;
        }
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                read_errors.push(format!("{relative}: {error}"));
                continue;
            }
        };
        read += 1;
        bytes += source.len();
        hash.update(&(relative.len() as u64).to_le_bytes());
        hash.update(relative.as_bytes());
        hash.update(&(source.len() as u64).to_le_bytes());
        hash.update(source.as_bytes());
        match extractor.extract_from_source(Language::Rust, &relative, &source) {
            Ok(extracted) => {
                parsed += 1;
                symbols += extracted.len();
            }
            Err(error) => {
                let mut tags = Vec::new();
                if source.contains("#[cfg") {
                    tags.push("cfg_present");
                }
                if source.contains(" for ") && source.contains("impl") {
                    tags.push("trait_impl_context_present");
                }
                blocked.push(format!(
                    "{{\"path\":{},\"error\":{},\"context_tags\":[{}]}}",
                    quoted(&relative),
                    quoted(&format!("{error:#}")),
                    tags.into_iter().map(quoted).collect::<Vec<_>>().join(",")
                ));
            }
        }
    }
    println!(
        "{{\"schema_version\":1,\"hash_algorithm\":\"BLAKE3(length-prefixed sorted relative path, UTF-8 source)\",\"source_hash\":{},\"rust_files_read\":{read},\"parsed_files\":{parsed},\"parsed_symbols\":{symbols},\"read_bytes\":{bytes},\"skipped_over_bounds\":{skipped},\"blocked_files\":[{}],\"read_errors\":[{}],\"limitations\":\"First extraction error per file; context tags are hints, not legal-source or collision classifications. No cfg evaluation, index/store access, or providers.\"}}",
        quoted(hash.finalize().to_hex().as_ref()),
        blocked.join(","),
        read_errors
            .iter()
            .map(|error| quoted(error))
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(())
}
