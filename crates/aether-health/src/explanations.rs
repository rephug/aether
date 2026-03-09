const FILE_LOC_TEMPLATE: &str =
    "{context} is {value} lines - large files are harder to navigate and reason about";
const TRAIT_METHOD_TEMPLATE: &str = "{context} has {value} methods - interfaces this large are harder to implement, test, and evolve independently";
const INTERNAL_DEP_TEMPLATE: &str =
    "{context} depends on {value} internal crates - high fan-in means changes propagate widely";
const TODO_DENSITY_TEMPLATE: &str = "{context} has TODO/FIXME density {value} per 1000 LOC - unresolved cleanup markers accumulate maintenance debt";
const DEAD_FEATURE_TEMPLATE: &str = "{context} still references {value} legacy feature flags - dormant code paths increase maintenance cost";
const STALE_REF_TEMPLATE: &str = "{context} still has {value} stale backend references in non-test code - migration cleanup is incomplete";

pub fn explain_violation(metric: &str, value: f64, _threshold: f64, context: &str) -> String {
    let value_text = format_value(value);
    let template = match metric {
        "max_file_loc" => FILE_LOC_TEMPLATE,
        "trait_method_max" => TRAIT_METHOD_TEMPLATE,
        "internal_dep_count" => INTERNAL_DEP_TEMPLATE,
        "todo_density" => TODO_DENSITY_TEMPLATE,
        "dead_feature_flags" => DEAD_FEATURE_TEMPLATE,
        "stale_backend_refs" => STALE_REF_TEMPLATE,
        _ => "{context} has value {value}",
    };

    template
        .replace("{context}", context)
        .replace("{value}", &value_text)
}

fn format_value(value: f64) -> String {
    if (value.fract()).abs() <= f64::EPSILON {
        format!("{}", value as i64)
    } else {
        format!("{value:.1}")
    }
}
