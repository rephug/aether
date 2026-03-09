use std::path::Path;

use aether_config::HealthScoreConfig;
use aether_health::{compute_workspace_score, format_json};

#[test]
fn score_on_real_workspace_and_json_output_valid() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");

    let report = compute_workspace_score(workspace_root, &HealthScoreConfig::default())
        .expect("workspace score");

    assert!(report.workspace_score > 0);
    assert!(report.crate_count >= 14);

    let json = format_json(&report);
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(parsed["schema_version"], 1);
    assert!(parsed["workspace_score"].as_u64().unwrap_or_default() > 0);
    assert!(
        parsed["crates"]
            .as_array()
            .map(|crates| crates.len() >= 14)
            .unwrap_or(false)
    );
}
