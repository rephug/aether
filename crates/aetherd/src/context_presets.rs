use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::cli::{ContextArgs, PresetArgs, PresetCommand};

pub const DEFAULT_CONTEXT_BUDGET: usize = 32_000;
pub const DEFAULT_CONTEXT_DEPTH: u32 = 2;
pub const DEFAULT_CONTEXT_FORMAT: &str = "markdown";
pub const DEFAULT_CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetConfig {
    pub preset: PresetMeta,
    pub context: PresetContextSettings,
    #[serde(default)]
    pub task_template: Option<PresetTaskTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetContextSettings {
    #[serde(default = "default_budget")]
    pub budget: usize,
    #[serde(default = "default_depth")]
    pub depth: u32,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_context_lines")]
    pub context_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresetTaskTemplate {
    pub template: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedContextOptions {
    pub format: String,
    pub budget: usize,
    pub depth: u32,
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub task: Option<String>,
    pub context_lines: usize,
}

pub fn default_budget() -> usize {
    DEFAULT_CONTEXT_BUDGET
}

pub fn default_depth() -> u32 {
    DEFAULT_CONTEXT_DEPTH
}

pub fn default_format() -> String {
    DEFAULT_CONTEXT_FORMAT.to_owned()
}

pub fn default_context_lines() -> usize {
    DEFAULT_CONTEXT_LINES
}

pub fn builtin_presets() -> Vec<PresetConfig> {
    vec![
        PresetConfig {
            preset: PresetMeta {
                name: "quick".to_owned(),
                description: "Quick question about a symbol".to_owned(),
            },
            context: PresetContextSettings {
                budget: 8_000,
                depth: 1,
                include: vec!["sir".to_owned(), "source".to_owned()],
                exclude: Vec::new(),
                format: default_format(),
                context_lines: default_context_lines(),
            },
            task_template: None,
        },
        PresetConfig {
            preset: PresetMeta {
                name: "review".to_owned(),
                description: "Code review context".to_owned(),
            },
            context: PresetContextSettings {
                budget: 32_000,
                depth: 2,
                include: vec![
                    "sir".to_owned(),
                    "source".to_owned(),
                    "graph".to_owned(),
                    "coupling".to_owned(),
                    "health".to_owned(),
                    "tests".to_owned(),
                ],
                exclude: Vec::new(),
                format: default_format(),
                context_lines: default_context_lines(),
            },
            task_template: None,
        },
        PresetConfig {
            preset: PresetMeta {
                name: "deep".to_owned(),
                description: "Deep analysis or refactor planning".to_owned(),
            },
            context: PresetContextSettings {
                budget: 64_000,
                depth: 3,
                include: vec![
                    "sir".to_owned(),
                    "source".to_owned(),
                    "graph".to_owned(),
                    "coupling".to_owned(),
                    "health".to_owned(),
                    "drift".to_owned(),
                    "memory".to_owned(),
                    "tests".to_owned(),
                ],
                exclude: Vec::new(),
                format: default_format(),
                context_lines: default_context_lines(),
            },
            task_template: None,
        },
        PresetConfig {
            preset: PresetMeta {
                name: "overview".to_owned(),
                description: "Project-level health check".to_owned(),
            },
            context: PresetContextSettings {
                budget: 16_000,
                depth: 0,
                include: vec!["sir".to_owned(), "health".to_owned(), "drift".to_owned()],
                exclude: Vec::new(),
                format: default_format(),
                context_lines: default_context_lines(),
            },
            task_template: None,
        },
    ]
}

pub fn list_presets(workspace: &Path) -> Result<Vec<PresetConfig>> {
    let mut by_name = builtin_presets()
        .into_iter()
        .map(|preset| (preset.preset.name.clone(), preset))
        .collect::<BTreeMap<_, _>>();

    for preset in read_user_presets(workspace)? {
        by_name.insert(preset.preset.name.clone(), preset);
    }

    Ok(by_name.into_values().collect())
}

pub fn load_preset(workspace: &Path, name: &str) -> Result<PresetConfig> {
    let normalized = normalize_preset_name(name)?;
    list_presets(workspace)?
        .into_iter()
        .find(|preset| preset.preset.name == normalized)
        .ok_or_else(|| anyhow!("unknown preset '{normalized}'"))
}

pub fn resolve_context_options(
    workspace: &Path,
    args: &ContextArgs,
) -> Result<ResolvedContextOptions> {
    let preset = args
        .preset
        .as_deref()
        .map(|name| load_preset(workspace, name))
        .transpose()?;

    let format = args
        .format
        .clone()
        .or_else(|| preset.as_ref().map(|value| value.context.format.clone()))
        .unwrap_or_else(default_format);
    validate_context_format(format.as_str())?;

    let budget = args
        .budget
        .or_else(|| preset.as_ref().map(|value| value.context.budget))
        .unwrap_or_else(default_budget);

    let depth = args
        .depth
        .or_else(|| preset.as_ref().map(|value| value.context.depth))
        .unwrap_or_else(default_depth);
    validate_depth(depth)?;

    let context_lines = args
        .context_lines
        .or_else(|| preset.as_ref().map(|value| value.context.context_lines))
        .unwrap_or_else(default_context_lines);

    let include = args.include.clone().or_else(|| {
        preset
            .as_ref()
            .and_then(|value| join_csv(value.context.include.as_slice()))
    });
    let exclude = args.exclude.clone().or_else(|| {
        preset
            .as_ref()
            .and_then(|value| join_csv(value.context.exclude.as_slice()))
    });
    let task = match args.task.clone().and_then(trimmed_option) {
        Some(task) => Some(task),
        None => preset_task(workspace, args, preset.as_ref())?.and_then(trimmed_option),
    };

    Ok(ResolvedContextOptions {
        format,
        budget,
        depth,
        include: include.and_then(trimmed_option),
        exclude: exclude.and_then(trimmed_option),
        task,
        context_lines,
    })
}

pub fn run_preset_command(workspace: &Path, args: PresetArgs) -> Result<()> {
    match args.command {
        PresetCommand::List => {
            let rendered = render_preset_list(&list_presets(workspace)?);
            let mut out = std::io::stdout();
            out.write_all(rendered.as_bytes())
                .context("failed to write preset list")?;
        }
        PresetCommand::Show(args) => {
            let preset = load_preset(workspace, args.name.as_str())?;
            let rendered = render_preset_toml(&preset)?;
            let mut out = std::io::stdout();
            out.write_all(rendered.as_bytes())
                .context("failed to write preset")?;
        }
        PresetCommand::Create(args) => {
            let path = create_user_preset(workspace, args.name.as_str())?;
            println!("Created {}", path.display());
        }
        PresetCommand::Delete(args) => {
            let path = delete_user_preset(workspace, args.name.as_str())?;
            println!("Deleted {}", path.display());
        }
    }
    Ok(())
}

pub fn render_preset_list(presets: &[PresetConfig]) -> String {
    let mut out = String::new();
    for preset in presets {
        let description = preset.preset.description.trim();
        if description.is_empty() {
            out.push_str(&format!("{}\n", preset.preset.name));
        } else {
            out.push_str(&format!("{}\t{}\n", preset.preset.name, description));
        }
    }
    out
}

pub fn render_preset_toml(preset: &PresetConfig) -> Result<String> {
    toml::to_string_pretty(preset).context("failed to serialize preset")
}

pub fn create_user_preset(workspace: &Path, name: &str) -> Result<PathBuf> {
    let normalized = normalize_preset_name(name)?;
    let path = user_preset_path(workspace, normalized.as_str());
    if path.exists() {
        return Err(anyhow!("preset already exists: {normalized}"));
    }

    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid preset path {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create preset directory {}", parent.display()))?;

    fs::write(&path, scaffold_preset_file(normalized.as_str()))
        .with_context(|| format!("failed to write preset {}", path.display()))?;
    Ok(path)
}

pub fn delete_user_preset(workspace: &Path, name: &str) -> Result<PathBuf> {
    let normalized = normalize_preset_name(name)?;
    let path = user_preset_path(workspace, normalized.as_str());
    if !path.exists() {
        if builtin_presets()
            .iter()
            .any(|preset| preset.preset.name == normalized)
        {
            return Err(anyhow!(
                "cannot delete built-in preset '{normalized}'; remove or override a user preset file instead"
            ));
        }
        return Err(anyhow!("user preset not found: {normalized}"));
    }

    fs::remove_file(&path)
        .with_context(|| format!("failed to delete preset {}", path.display()))?;
    Ok(path)
}

fn preset_task(
    workspace: &Path,
    args: &ContextArgs,
    preset: Option<&PresetConfig>,
) -> Result<Option<String>> {
    let Some(preset) = preset else {
        return Ok(None);
    };
    let Some(template) = preset.task_template.as_ref() else {
        return Ok(None);
    };

    let target = context_target_label(workspace, args)?;
    Ok(Some(template.template.replace("{target}", target.as_str())))
}

fn context_target_label(workspace: &Path, args: &ContextArgs) -> Result<String> {
    if args.overview {
        return Ok("workspace overview".to_owned());
    }
    if let Some(branch) = args.branch.as_deref().and_then(trimmed_str) {
        return Ok(format!("branch:{branch}"));
    }
    if let Some(symbol) = args.symbol.as_deref().and_then(trimmed_str) {
        return Ok(symbol.to_owned());
    }

    let mut labels = Vec::new();
    for target in &args.targets {
        labels.push(normalize_workspace_relative_path(workspace, target)?);
    }
    if labels.is_empty() {
        return Ok("workspace overview".to_owned());
    }
    labels.sort();
    labels.dedup();
    Ok(labels.join(", "))
}

fn validate_context_format(value: &str) -> Result<()> {
    match value.trim() {
        "markdown" | "json" | "xml" | "compact" => Ok(()),
        other => Err(anyhow!(
            "unsupported context output format '{other}', expected one of: markdown, json, xml, compact"
        )),
    }
}

fn validate_depth(value: u32) -> Result<()> {
    if value <= 3 {
        Ok(())
    } else {
        Err(anyhow!("invalid preset depth {value}, expected 0..=3"))
    }
}

fn read_user_presets(workspace: &Path) -> Result<Vec<PresetConfig>> {
    let directory = workspace.join(".aether/presets");
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(&directory)
        .with_context(|| format!("failed to read preset directory {}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read preset directory {}", directory.display()))?;
    entries.sort_by_key(|entry| entry.path());

    let mut presets = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read preset {}", path.display()))?;
        let preset = parse_preset(raw.as_str(), &path)?;
        presets.push(preset);
    }
    Ok(presets)
}

fn parse_preset(raw: &str, path: &Path) -> Result<PresetConfig> {
    let preset = toml::from_str::<PresetConfig>(raw)
        .with_context(|| format!("failed to parse preset {}", path.display()))?;
    validate_preset(&preset, path)?;
    Ok(preset)
}

fn validate_preset(preset: &PresetConfig, path: &Path) -> Result<()> {
    let name = normalize_preset_name(preset.preset.name.as_str())
        .with_context(|| format!("invalid preset name in {}", path.display()))?;
    if name != preset.preset.name {
        return Err(anyhow!(
            "preset name '{}' in {} must be normalized as '{}'",
            preset.preset.name,
            path.display(),
            name
        ));
    }

    validate_context_format(preset.context.format.as_str())
        .with_context(|| format!("invalid format in {}", path.display()))?;
    validate_depth(preset.context.depth)
        .with_context(|| format!("invalid depth in {}", path.display()))?;
    Ok(())
}

fn scaffold_preset_file(name: &str) -> String {
    let layers = [
        "sir", "source", "graph", "coupling", "health", "drift", "memory", "tests",
    ]
    .into_iter()
    .map(|value| format!("\"{value}\""))
    .collect::<Vec<_>>()
    .join(", ");

    format!(
        "[preset]\nname = \"{name}\"\ndescription = \"\"  # Add a description\n\n[context]\nbudget = {budget}\ndepth = {depth}\ninclude = [{layers}]\n# exclude = []\nformat = \"{format}\"\ncontext_lines = {context_lines}\n\n# [task_template]\n# template = \"Plan a refactor of {{target}}\"\n",
        budget = default_budget(),
        depth = default_depth(),
        format = default_format(),
        context_lines = default_context_lines(),
    )
}

fn user_preset_path(workspace: &Path, name: &str) -> PathBuf {
    workspace
        .join(".aether/presets")
        .join(format!("{name}.toml"))
}

fn normalize_preset_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("preset name must not be empty"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err(anyhow!("preset name must not contain path separators"));
    }
    Ok(trimmed.to_owned())
}

fn join_csv(values: &[String]) -> Option<String> {
    let joined = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if joined.is_empty() {
        None
    } else {
        Some(joined.join(","))
    }
}

fn normalize_workspace_relative_path(workspace: &Path, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("target path must not be empty"));
    }

    let path = Path::new(trimmed);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let canonical = absolute
        .canonicalize()
        .with_context(|| format!("failed to resolve target path {}", absolute.display()))?;
    let relative = canonical.strip_prefix(workspace).with_context(|| {
        format!(
            "target path {} is not inside workspace {}",
            canonical.display(),
            workspace.display()
        )
    })?;

    Ok(relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned())
}

fn trimmed_option(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn trimmed_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        DEFAULT_CONTEXT_LINES, builtin_presets, create_user_preset, delete_user_preset,
        list_presets, load_preset, resolve_context_options,
    };
    use crate::cli::ContextArgs;

    fn context_args() -> ContextArgs {
        ContextArgs {
            targets: vec!["src/lib.rs".to_owned()],
            symbol: None,
            file: None,
            overview: false,
            branch: None,
            preset: None,
            format: None,
            budget: None,
            depth: None,
            include: None,
            exclude: None,
            task: None,
            context_lines: None,
            output: None,
        }
    }

    #[test]
    fn builtin_presets_cover_expected_names() {
        let names = builtin_presets()
            .into_iter()
            .map(|preset| preset.preset.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["quick", "review", "deep", "overview"]);
    }

    #[test]
    fn user_preset_overrides_builtin() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".aether/presets")).expect("create preset dir");
        fs::write(
            temp.path().join(".aether/presets/deep.toml"),
            r#"
[preset]
name = "deep"
description = "overridden"

[context]
budget = 1234
depth = 1
include = ["sir"]
format = "compact"
context_lines = 7
"#,
        )
        .expect("write preset");

        let deep = load_preset(temp.path(), "deep").expect("load preset");
        assert_eq!(deep.context.budget, 1234);
        assert_eq!(deep.context.format, "compact");
    }

    #[test]
    fn invalid_preset_toml_reports_path() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".aether/presets")).expect("create preset dir");
        let path = temp.path().join(".aether/presets/bad.toml");
        fs::write(&path, "[preset\nname = \"bad\"").expect("write invalid preset");

        let err = list_presets(temp.path()).expect_err("invalid toml should fail");
        assert!(
            err.to_string()
                .contains(path.display().to_string().as_str())
        );
    }

    #[test]
    fn context_resolution_applies_preset_then_cli_override() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".aether/presets")).expect("create preset dir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::write(temp.path().join("src/lib.rs"), "pub fn alpha() {}\n").expect("write source");
        fs::write(
            temp.path().join(".aether/presets/deep.toml"),
            r#"
[preset]
name = "deep"

[context]
budget = 64000
depth = 3
include = ["sir", "source", "graph", "coupling", "health", "drift", "memory", "tests"]
format = "markdown"
context_lines = 4
"#,
        )
        .expect("write preset");

        let mut args = context_args();
        args.preset = Some("deep".to_owned());
        args.budget = Some(16_000);

        let resolved = resolve_context_options(temp.path(), &args).expect("resolve options");
        assert_eq!(resolved.budget, 16_000);
        assert_eq!(resolved.depth, 3);
        assert_eq!(resolved.context_lines, 4);
        assert_eq!(
            resolved.include.as_deref(),
            Some("sir,source,graph,coupling,health,drift,memory,tests")
        );
    }

    #[test]
    fn task_template_substitutes_targets() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join(".aether/presets")).expect("create preset dir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::write(temp.path().join("src/lib.rs"), "pub fn alpha() {}\n").expect("write source");
        fs::write(
            temp.path().join(".aether/presets/review.toml"),
            r#"
[preset]
name = "review"

[context]
budget = 32000
depth = 2
include = ["sir", "source"]
format = "markdown"
context_lines = 3

[task_template]
template = "Plan changes for {target}"
"#,
        )
        .expect("write preset");

        let mut args = context_args();
        args.preset = Some("review".to_owned());

        let resolved = resolve_context_options(temp.path(), &args).expect("resolve options");
        assert_eq!(
            resolved.task.as_deref(),
            Some("Plan changes for src/lib.rs")
        );
    }

    #[test]
    fn create_and_delete_user_preset_manage_files() {
        let temp = tempdir().expect("tempdir");
        let created = create_user_preset(temp.path(), "refactor-plan").expect("create preset");
        let contents = fs::read_to_string(&created).expect("read preset");
        assert!(contents.contains("context_lines = 3"));
        assert!(contents.contains("# [task_template]"));

        let deleted = delete_user_preset(temp.path(), "refactor-plan").expect("delete preset");
        assert_eq!(created, deleted);
        assert!(!deleted.exists());
    }

    #[test]
    fn default_resolution_uses_expected_defaults() {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::write(temp.path().join("src/lib.rs"), "pub fn alpha() {}\n").expect("write source");

        let resolved = resolve_context_options(temp.path(), &context_args()).expect("resolve");
        assert_eq!(resolved.format, "markdown");
        assert_eq!(resolved.budget, 32_000);
        assert_eq!(resolved.depth, 2);
        assert_eq!(resolved.context_lines, DEFAULT_CONTEXT_LINES);
    }
}
