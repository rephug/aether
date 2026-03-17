use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::api::batch;
use crate::support::{self, DashboardState};

pub(crate) async fn batch_fragment(State(state): State<Arc<DashboardState>>) -> Html<String> {
    let shared = state.shared.clone();
    let data = support::run_blocking_with_timeout(move || batch::load_batch_data(shared.as_ref()))
        .await
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "dashboard: failed to load batch fragment data");
            batch::BatchData {
                has_data: false,
                last_run_at: None,
                written_requests: 0,
                skipped_requests: 0,
                chunk_count: 0,
                auto_submit: false,
                submitted_chunks: 0,
                ingested_results: 0,
                fingerprint_rows: 0,
                requeue_pass: String::new(),
                last_error: None,
                batch_files: Vec::new(),
            }
        });

    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Batch Processing Pipeline",
                "The batch pipeline generates and updates AETHER's understanding of your code in bulk.",
                "Batch processing uses the Gemini Batch API to regenerate SIRs for many symbols at once.",
                "Gemini Batch API pipeline: extract → build JSONL → submit → poll → ingest results."
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Batch Processing Pipeline" }
                    span class="intermediate-only" { "Batch Pipeline Status" }
                    span class="expert-only" { "Batch Pipeline" }
                }
                @if data.has_data {
                    span class="badge badge-cyan" { "Last run: " (support::format_age_seconds(data.last_run_at.map(now_minus))) " ago" }
                } @else {
                    span class="badge badge-muted" { "No runs yet" }
                }
            }

            @if !data.has_data {
                (support::html_empty_state("No batch runs yet", Some("aether batch run")))
            } @else {
                // Status card
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-3" {
                    h3 class="text-sm font-semibold" { "Last Run Summary" }
                    div class="grid grid-cols-2 md:grid-cols-4 gap-3" {
                        (metric_card("Requests Written", &data.written_requests.to_string()))
                        (metric_card("Skipped (cached)", &data.skipped_requests.to_string()))
                        (metric_card("Chunks Submitted", &data.submitted_chunks.to_string()))
                        (metric_card("Results Ingested", &data.ingested_results.to_string()))
                    }

                    div class="flex flex-wrap gap-2 text-xs" {
                        span class="badge badge-muted" { "Fingerprint rows: " (data.fingerprint_rows) }
                        span class="badge badge-muted" { "Chunks: " (data.chunk_count) }
                        @if !data.requeue_pass.is_empty() {
                            span class="badge badge-cyan" { "Pass: " (data.requeue_pass) }
                        }
                        @if data.auto_submit {
                            span class="badge badge-green" { "Auto-submit: on" }
                        } @else {
                            span class="badge badge-yellow" { "Auto-submit: off" }
                        }
                    }
                }

                // Error display
                @if let Some(error) = &data.last_error {
                    div class="rounded-xl border border-red-300/40 bg-red-50/40 dark:border-red-700/40 dark:bg-red-950/20 p-4" {
                        h3 class="text-sm font-semibold text-red-700 dark:text-red-400" { "Last Error" }
                        pre class="text-xs text-red-600 dark:text-red-300 mt-1 whitespace-pre-wrap" { (error) }
                    }
                }

                // Batch files
                @if !data.batch_files.is_empty() {
                    div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4 space-y-2" {
                        h3 class="text-sm font-semibold" { "Batch Files" }
                        table class="data-table" {
                            thead {
                                tr {
                                    th { "Filename" }
                                    th { "Size" }
                                }
                            }
                            tbody {
                                @for file in &data.batch_files {
                                    tr {
                                        td class="font-mono text-xs" { (file.filename.as_str()) }
                                        td class="font-mono text-xs" { (format_bytes(file.size_bytes)) }
                                    }
                                }
                            }
                        }
                    }
                }

                // CLI hint
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-4" {
                    h3 class="text-sm font-semibold" { "Run Batch" }
                    p class="text-xs text-text-secondary mt-1" { "To start a new batch run, use the CLI:" }
                    code class="block mt-2 text-xs font-mono bg-surface-2/60 dark:bg-slate-800/60 p-2 rounded select-all" {
                        "aether batch run"
                    }
                }
            }
        }
    })
}

fn metric_card(label: &str, value: &str) -> maud::Markup {
    html! {
        div class="rounded-lg border border-surface-3/30 bg-surface-0/60 dark:bg-slate-800/40 p-3 text-center" {
            div class="text-xl font-bold" { (value) }
            div class="text-xs text-text-muted mt-1" { (label) }
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1} KB");
    }
    let mb = kb / 1024.0;
    format!("{mb:.1} MB")
}

fn now_minus(timestamp: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (now - timestamp).max(0)
}
