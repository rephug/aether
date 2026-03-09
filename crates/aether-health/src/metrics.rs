use toml::Value;

pub fn count_loc(file_content: &str) -> (usize, usize) {
    let mut non_blank_non_comment = 0;
    let mut total_lines = 0;
    let mut block_depth = 0usize;

    for line in file_content.lines() {
        total_lines += 1;
        if has_code_content(line, &mut block_depth) {
            non_blank_non_comment += 1;
        }
    }

    (non_blank_non_comment, total_lines)
}

pub fn count_trait_methods(file_content: &str) -> usize {
    trait_method_max(file_content).0
}

pub(crate) fn trait_method_max(file_content: &str) -> (usize, Option<String>) {
    let mut max_methods = 0usize;
    let mut max_trait_name = None;
    let mut pending_trait_name: Option<String> = None;
    let mut current_trait_name: Option<String> = None;
    let mut current_method_count = 0usize;
    let mut brace_depth = 0usize;

    for line in file_content.lines() {
        let trimmed = line.trim_start();

        if let Some(name) = parse_pub_trait_name(trimmed) {
            pending_trait_name = Some(name);
            if let Some(open_brace) = line.find('{') {
                current_trait_name = pending_trait_name.take();
                brace_depth = brace_delta(&line[open_brace..]);
                current_method_count = count_trait_function_in_segment(&line[open_brace + 1..]);
                if brace_depth == 0 {
                    update_trait_max(
                        current_trait_name.take(),
                        current_method_count,
                        &mut max_methods,
                        &mut max_trait_name,
                    );
                    current_method_count = 0;
                }
            }
            continue;
        }

        if current_trait_name.is_none() {
            if pending_trait_name.is_some() && line.contains('{') {
                current_trait_name = pending_trait_name.take();
                if let Some(open_brace) = line.find('{') {
                    brace_depth = brace_delta(&line[open_brace..]);
                    current_method_count = count_trait_function_in_segment(&line[open_brace + 1..]);
                    if brace_depth == 0 {
                        update_trait_max(
                            current_trait_name.take(),
                            current_method_count,
                            &mut max_methods,
                            &mut max_trait_name,
                        );
                        current_method_count = 0;
                    }
                }
            } else if pending_trait_name.is_some() && trimmed.ends_with(';') {
                pending_trait_name = None;
            }
            continue;
        }

        current_method_count += count_trait_function_in_segment(trimmed);
        brace_depth = update_brace_depth(brace_depth, line);
        if brace_depth == 0 {
            update_trait_max(
                current_trait_name.take(),
                current_method_count,
                &mut max_methods,
                &mut max_trait_name,
            );
            current_method_count = 0;
        }
    }

    (max_methods, max_trait_name)
}

pub fn count_stale_refs(file_content: &str, patterns: &[String]) -> usize {
    let mut count = 0usize;
    let mut pending_skip = false;
    let mut skip_depth = 0usize;

    for line in file_content.lines() {
        let trimmed = line.trim_start();

        if skip_depth > 0 {
            skip_depth = update_brace_depth(skip_depth, line);
            continue;
        }

        if is_skip_cfg_attribute(trimmed) {
            pending_skip = true;
            continue;
        }

        if pending_skip {
            if let Some(open_brace) = line.find('{') {
                skip_depth = brace_delta(&line[open_brace..]);
                pending_skip = false;
                continue;
            }
            if trimmed.ends_with(';') {
                pending_skip = false;
            }
            continue;
        }

        for pattern in patterns {
            count += line.matches(pattern).count();
        }
    }

    count
}

pub fn count_feature_flags(file_content: &str, pattern: &str) -> usize {
    file_content.matches(pattern).count()
}

pub fn count_todo_density(file_content: &str) -> f32 {
    let (non_blank_non_comment, _) = count_loc(file_content);
    if non_blank_non_comment == 0 {
        return 0.0;
    }

    (count_todo_markers(file_content) as f32 * 1000.0) / non_blank_non_comment as f32
}

pub(crate) fn count_todo_markers(file_content: &str) -> usize {
    file_content.matches("TODO").count() + file_content.matches("FIXME").count()
}

pub fn count_internal_deps(cargo_toml: &Value) -> usize {
    let mut deps = Vec::new();
    collect_dependency_names(cargo_toml.get("dependencies"), &mut deps);
    collect_dependency_names(cargo_toml.get("dev-dependencies"), &mut deps);
    collect_dependency_names(cargo_toml.get("build-dependencies"), &mut deps);

    if let Some(targets) = cargo_toml.get("target").and_then(Value::as_table) {
        for target in targets.values() {
            collect_dependency_names(target.get("dependencies"), &mut deps);
            collect_dependency_names(target.get("dev-dependencies"), &mut deps);
            collect_dependency_names(target.get("build-dependencies"), &mut deps);
        }
    }

    deps.sort();
    deps.dedup();
    deps.len()
}

fn collect_dependency_names(value: Option<&Value>, deps: &mut Vec<String>) {
    let Some(table) = value.and_then(Value::as_table) else {
        return;
    };

    for key in table.keys() {
        if key.starts_with("aether-") {
            deps.push(key.clone());
        }
    }
}

fn has_code_content(line: &str, block_depth: &mut usize) -> bool {
    let mut output = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        let current = chars[index];
        let next = chars.get(index + 1).copied();

        if *block_depth > 0 {
            if current == '*' && next == Some('/') {
                *block_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if current == '/' && next == Some('/') {
            break;
        }

        if current == '/' && next == Some('*') {
            *block_depth += 1;
            index += 2;
            continue;
        }

        output.push(current);
        index += 1;
    }

    !output.trim().is_empty()
}

fn parse_pub_trait_name(line: &str) -> Option<String> {
    let remainder = line.strip_prefix("pub trait ")?;
    let mut trait_name = String::new();
    for ch in remainder.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            trait_name.push(ch);
        } else {
            break;
        }
    }

    if trait_name.is_empty() {
        None
    } else {
        Some(trait_name)
    }
}

fn count_trait_function_in_segment(segment: &str) -> usize {
    let mut remainder = segment.trim_start();
    while let Some(stripped) = remainder.strip_prefix("async ") {
        remainder = stripped.trim_start();
    }
    while let Some(stripped) = remainder.strip_prefix("unsafe ") {
        remainder = stripped.trim_start();
    }

    usize::from(remainder.starts_with("fn "))
}

fn update_trait_max(
    trait_name: Option<String>,
    method_count: usize,
    max_methods: &mut usize,
    max_trait_name: &mut Option<String>,
) {
    if method_count > *max_methods {
        *max_methods = method_count;
        *max_trait_name = trait_name;
    }
}

fn is_skip_cfg_attribute(trimmed: &str) -> bool {
    trimmed.starts_with("#[cfg(test)]")
        || trimmed.contains("cfg(test)")
        || trimmed.contains("cfg(feature = \"legacy-cozo\")")
}

fn brace_delta(segment: &str) -> usize {
    let mut depth = 0usize;
    for ch in segment.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn update_brace_depth(current_depth: usize, line: &str) -> usize {
    let mut depth = current_depth;
    for ch in line.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::{
        count_feature_flags, count_internal_deps, count_loc, count_stale_refs, count_todo_density,
        count_trait_methods, trait_method_max,
    };

    #[test]
    fn trait_method_counter_accuracy() {
        let source = r#"
pub trait Store {
    fn alpha(&self);
    async fn beta(&self);
}

pub trait Small {
    fn single(&self);
}
"#;

        assert_eq!(count_trait_methods(source), 2);
        assert_eq!(trait_method_max(source).1.as_deref(), Some("Store"));
    }

    #[test]
    fn stale_ref_excludes_test_modules() {
        let source = r#"
const ACTIVE: &str = "cozo";

#[cfg(test)]
mod tests {
    const LEGACY: &str = "cozo";
}
"#;

        assert_eq!(count_stale_refs(source, &[String::from("cozo")]), 1);
    }

    #[test]
    fn loc_counter_excludes_comments_blanks() {
        let source = r#"
// comment

fn alpha() {}
/* block
comment */
fn beta() {}
"#;

        assert_eq!(count_loc(source), (2, 7));
    }

    #[test]
    fn count_feature_flags_finds_matches() {
        assert_eq!(
            count_feature_flags("#[cfg(feature = \"legacy-cozo\")]", "feature = \"legacy-"),
            1
        );
    }

    #[test]
    fn count_todo_density_uses_non_comment_loc() {
        let source = r#"
// TODO ignored for denominator
fn alpha() {} // TODO
fn beta() {} // FIXME
"#;

        let density = count_todo_density(source);
        assert!(density > 900.0);
    }

    #[test]
    fn count_internal_deps_aggregates_unique_sections() {
        let manifest: toml::Value = toml::from_str(
            r#"
[dependencies]
aether-config = { path = "../aether-config" }

[dev-dependencies]
aether-config = { path = "../aether-config" }
aether-store = { path = "../aether-store" }

[target.'cfg(test)'.dependencies]
aether-store = { path = "../aether-store" }
aether-health = { path = "../aether-health" }
"#,
        )
        .expect("manifest");

        assert_eq!(count_internal_deps(&manifest), 3);
    }
}
