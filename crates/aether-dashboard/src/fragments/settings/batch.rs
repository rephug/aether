use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let batch = match config.batch.as_ref() {
        Some(b) => b,
        None => {
            return html! {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                    h3 class="text-sm font-semibold text-text-primary pb-2" { "Batch Pipeline Settings" }
                    p class="text-sm text-text-muted" {
                        "Batch pipeline is not configured. Add a "
                        code class="text-xs bg-surface-3/30 px-1 py-0.5 rounded" { "[batch]" }
                        " section to your config.toml to enable batch processing."
                    }
                }
            };
        }
    };

    let thinking_options: &[(&str, &str)] = &[
        ("none", "None"),
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
    ];

    html! {
        form hx-post="/api/v1/settings/batch"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Batch Pipeline Settings" }

            (helpers::section_divider("Models"))

            (helpers::text_input(
                "scan_model",
                "Scan Model",
                &batch.scan_model,
                "Model for scan pass batch requests",
            ))

            (helpers::text_input(
                "triage_model",
                "Triage Model",
                &batch.triage_model,
                "Model for triage pass batch requests",
            ))

            (helpers::text_input(
                "deep_model",
                "Deep Model",
                &batch.deep_model,
                "Model for deep pass batch requests",
            ))

            (helpers::section_divider("Thinking Levels"))

            (helpers::select_input(
                "scan_thinking",
                "Scan Thinking",
                &batch.scan_thinking,
                thinking_options,
                "Thinking level for scan pass",
            ))

            (helpers::select_input(
                "triage_thinking",
                "Triage Thinking",
                &batch.triage_thinking,
                thinking_options,
                "Thinking level for triage pass",
            ))

            (helpers::select_input(
                "deep_thinking",
                "Deep Thinking",
                &batch.deep_thinking,
                thinking_options,
                "Thinking level for deep pass",
            ))

            (helpers::section_divider("Pipeline"))

            (helpers::toggle_input(
                "auto_chain",
                "Auto Chain",
                batch.auto_chain,
                "Automatically chain passes (scan \u{2192} triage \u{2192} deep)",
            ))

            (helpers::number_input(
                "jsonl_chunk_size",
                "JSONL Chunk Size",
                batch.jsonl_chunk_size,
                "Max requests per JSONL batch file",
                Some("1"),
                None,
                None,
            ))

            (helpers::number_input(
                "poll_interval_secs",
                "Poll Interval (seconds)",
                batch.poll_interval_secs,
                "Seconds between batch status polls",
                Some("1"),
                None,
                None,
            ))

            (helpers::save_reset_buttons("batch"))
        }
    }
}
