use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{
    MAX_SEARCH_THRESHOLD, MIN_SEARCH_THRESHOLD, ensure_workspace_config, save_workspace_config,
};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{CalibrationEmbeddingRecord, SqliteStore, Store, ThresholdCalibrationRecord};
use anyhow::{Result, anyhow};

const MIN_SYMBOLS_PER_LANGUAGE: usize = 20;
const MAX_INTRA_FILE_PAIRS: usize = 500;
const MAX_INTER_FILE_PAIRS: usize = 500;
const CALIBRATED_LANGUAGES: [&str; 4] = ["default", "rust", "typescript", "python"];

#[derive(Debug, Clone, PartialEq)]
struct LanguageCalibration {
    language: String,
    threshold: f32,
    sample_size: i64,
}

pub fn run_calibration_once(workspace: &Path) -> Result<()> {
    let mut config = ensure_workspace_config(workspace)?;
    let loaded = load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())?
        .ok_or_else(|| {
            anyhow!(
                "embeddings are disabled in .aether/config.toml; enable [embeddings].enabled=true before --calibrate"
            )
        })?;

    let store = SqliteStore::open(workspace)?;
    let rows =
        store.list_embeddings_for_provider_model(&loaded.provider_name, &loaded.model_name)?;
    if rows.is_empty() {
        println!(
            "No symbols to calibrate for provider={} model={}. Run indexing first.",
            loaded.provider_name, loaded.model_name
        );
        return Ok(());
    }

    let mut completed = Vec::new();
    for language in CALIBRATED_LANGUAGES {
        let bucket = language_bucket(&rows, language);
        let Some((threshold, sample_size)) = calibrate_threshold_for_language(&bucket) else {
            // Clear stale calibrated values when this run doesn't have enough data.
            config
                .search
                .calibrated_thresholds
                .set_for_language(language, None);
            continue;
        };

        let timestamp = calibration_timestamp();
        store.upsert_threshold_calibration(ThresholdCalibrationRecord {
            language: language.to_owned(),
            threshold,
            sample_size,
            provider: loaded.provider_name.clone(),
            model: loaded.model_name.clone(),
            calibrated_at: timestamp,
        })?;
        config
            .search
            .calibrated_thresholds
            .set_for_language(language, Some(threshold));

        completed.push(LanguageCalibration {
            language: language.to_owned(),
            threshold,
            sample_size,
        });
    }

    if completed.is_empty() {
        println!(
            "No language bucket had at least {MIN_SYMBOLS_PER_LANGUAGE} symbols for calibration. Using defaults."
        );
        save_workspace_config(workspace, &config)?;
        return Ok(());
    }

    save_workspace_config(workspace, &config)?;

    println!("Calibrated thresholds:");
    for entry in &completed {
        println!(
            "  {}: {:.3} (based on {} sampled pairs)",
            entry.language, entry.threshold, entry.sample_size
        );
    }
    println!("Written to .aether/config.toml [search.calibrated_thresholds]");

    Ok(())
}

fn language_bucket<'a>(
    rows: &'a [CalibrationEmbeddingRecord],
    language: &str,
) -> Vec<&'a CalibrationEmbeddingRecord> {
    let target = normalize_language(language);
    rows.iter()
        .filter(|row| {
            if target == "default" {
                return true;
            }
            normalize_language(&row.language) == target
        })
        .collect()
}

fn calibrate_threshold_for_language(rows: &[&CalibrationEmbeddingRecord]) -> Option<(f32, i64)> {
    if rows.len() < MIN_SYMBOLS_PER_LANGUAGE {
        return None;
    }

    let intra = sample_intra_file_similarities(rows, MAX_INTRA_FILE_PAIRS);
    let inter = sample_inter_file_similarities(rows, MAX_INTER_FILE_PAIRS);

    let threshold = threshold_from_distributions(&intra, &inter)?;
    let sample_size = (intra.len() + inter.len()) as i64;
    Some((threshold, sample_size))
}

fn sample_intra_file_similarities(rows: &[&CalibrationEmbeddingRecord], cap: usize) -> Vec<f32> {
    let mut by_file: HashMap<&str, Vec<&CalibrationEmbeddingRecord>> = HashMap::new();
    for row in rows {
        by_file
            .entry(row.file_path.as_str())
            .or_default()
            .push(*row);
    }

    let mut similarities = Vec::new();
    let mut file_groups = by_file.into_values().collect::<Vec<_>>();
    file_groups.sort_by(|left, right| {
        left[0]
            .file_path
            .cmp(&right[0].file_path)
            .then_with(|| left.len().cmp(&right.len()))
    });

    for group in file_groups {
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                if let Some(similarity) =
                    cosine_similarity(&group[i].embedding, &group[j].embedding)
                {
                    similarities.push(similarity);
                    if similarities.len() >= cap {
                        return similarities;
                    }
                }
            }
        }
    }

    similarities
}

fn sample_inter_file_similarities(rows: &[&CalibrationEmbeddingRecord], cap: usize) -> Vec<f32> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|left, right| {
        left.file_path
            .cmp(&right.file_path)
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });

    let mut similarities = Vec::new();
    for i in 0..sorted.len() {
        for j in (i + 1)..sorted.len() {
            if sorted[i].file_path == sorted[j].file_path {
                continue;
            }

            if let Some(similarity) = cosine_similarity(&sorted[i].embedding, &sorted[j].embedding)
            {
                similarities.push(similarity);
                if similarities.len() >= cap {
                    return similarities;
                }
            }
        }
    }

    similarities
}

fn threshold_from_distributions(intra: &[f32], inter: &[f32]) -> Option<f32> {
    if intra.is_empty() || inter.is_empty() {
        return None;
    }

    let mean_intra = mean(intra)?;
    let mean_inter = mean(inter)?;
    let midpoint = (mean_intra + mean_inter) / 2.0;
    Some(clamp_threshold(midpoint))
}

fn mean(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }

    let mut sum = 0.0f32;
    for value in values {
        if !value.is_finite() {
            return None;
        }
        sum += value;
    }

    Some(sum / values.len() as f32)
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }

    let mut dot = 0.0f32;
    let mut left_norm_sq = 0.0f32;
    let mut right_norm_sq = 0.0f32;

    for (left_value, right_value) in left.iter().zip(right.iter()) {
        dot += left_value * right_value;
        left_norm_sq += left_value * left_value;
        right_norm_sq += right_value * right_value;
    }

    if left_norm_sq <= f32::EPSILON || right_norm_sq <= f32::EPSILON {
        return None;
    }

    Some(dot / (left_norm_sq.sqrt() * right_norm_sq.sqrt()))
}

fn clamp_threshold(value: f32) -> f32 {
    if !value.is_finite() {
        return MIN_SEARCH_THRESHOLD;
    }

    value.clamp(MIN_SEARCH_THRESHOLD, MAX_SEARCH_THRESHOLD)
}

fn normalize_language(language: &str) -> &'static str {
    let normalized = language.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "rust" | "rs" => "rust",
        "typescript" | "ts" | "tsx" | "javascript" | "js" => "typescript",
        "python" | "py" => "python",
        _ => "default",
    }
}

fn calibration_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    seconds.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedding_record(
        symbol_id: &str,
        file_path: &str,
        language: &str,
        embedding: Vec<f32>,
    ) -> CalibrationEmbeddingRecord {
        CalibrationEmbeddingRecord {
            symbol_id: symbol_id.to_owned(),
            file_path: file_path.to_owned(),
            language: language.to_owned(),
            embedding,
        }
    }

    #[test]
    fn threshold_from_distribution_uses_midpoint() {
        let threshold =
            threshold_from_distributions(&[0.8, 0.9, 0.7], &[0.2, 0.1, 0.3]).expect("threshold");
        assert!((threshold - 0.5).abs() < 0.01);
    }

    #[test]
    fn threshold_is_clamped_to_bounds() {
        let high = threshold_from_distributions(&[1.4, 1.5], &[1.2, 1.1]).expect("high");
        let low = threshold_from_distributions(&[-2.0, -1.9], &[-1.8, -1.7]).expect("low");
        assert_eq!(high, MAX_SEARCH_THRESHOLD);
        assert_eq!(low, MIN_SEARCH_THRESHOLD);
    }

    #[test]
    fn calibration_skips_small_language_buckets() {
        let rows = (0..10)
            .map(|index| {
                embedding_record(
                    &format!("sym-{index}"),
                    "src/lib.rs",
                    "rust",
                    vec![1.0, 0.0],
                )
            })
            .collect::<Vec<_>>();
        let refs = rows.iter().collect::<Vec<_>>();
        assert!(calibrate_threshold_for_language(&refs).is_none());
    }

    #[test]
    fn calibration_builds_threshold_from_synthetic_embeddings() {
        let mut rows = Vec::new();
        for index in 0..12 {
            rows.push(embedding_record(
                &format!("a-{index}"),
                "src/a.rs",
                "rust",
                vec![1.0, 0.0],
            ));
            rows.push(embedding_record(
                &format!("b-{index}"),
                "src/b.rs",
                "rust",
                vec![0.0, 1.0],
            ));
        }

        let refs = rows.iter().collect::<Vec<_>>();
        let (threshold, sample_size) = calibrate_threshold_for_language(&refs).expect("calibrated");
        assert!(sample_size > 0);
        // Intra-file pairs are near 1.0 and inter-file pairs are near 0.0.
        assert!(threshold > 0.45 && threshold < 0.75);
    }

    #[test]
    fn language_bucket_supports_aliases() {
        let rows = vec![
            embedding_record("sym-1", "src/lib.rs", "rs", vec![1.0, 0.0]),
            embedding_record("sym-2", "src/jobs.py", "python", vec![0.0, 1.0]),
        ];

        let rust = language_bucket(&rows, "rust");
        let python = language_bucket(&rows, "python");
        let default_bucket = language_bucket(&rows, "default");
        assert_eq!(rust.len(), 1);
        assert_eq!(python.len(), 1);
        assert_eq!(default_bucket.len(), 2);
    }

    #[test]
    fn missing_distributions_do_not_produce_threshold() {
        assert!(threshold_from_distributions(&[0.7], &[]).is_none());
        assert!(threshold_from_distributions(&[], &[0.2]).is_none());
    }
}
