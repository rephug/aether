use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Html;
use maud::html;

use crate::api::spec;
use crate::support::{self, DashboardState};

pub(crate) async fn spec_fragment(
    State(state): State<Arc<DashboardState>>,
    Path(selector): Path<String>,
) -> Html<String> {
    let shared = state.shared.clone();
    let selector_for_build = selector.clone();
    let data = match support::run_blocking_with_timeout(move || {
        spec::build_spec_data(shared.as_ref(), selector_for_build.as_str())
    })
    .await
    {
        Ok(Some(data)) => data,
        Ok(None) => {
            return support::html_markup_response(html! {
                (support::html_empty_state("Symbol not found", None))
            });
        }
        Err(err) => {
            let detail = support::extract_timeout_error_message(err.as_str()).unwrap_or(err);
            return support::html_markup_response(html! {
                (support::html_error_state("Failed to generate spec", detail.as_str()))
            });
        }
    };

    let copy_payload = build_copy_payload(&data);

    support::html_markup_response(html! {
        div class="space-y-4" data-page="spec" {
            (support::explanation_header(
                "Code to Spec",
                "Generate a concrete implementation spec from existing SIR and dependency data.",
                "Use this output as a build-ready contract for coding agents.",
                "Template-composed spec from SIR intent, dependencies, and error modes."
            ))

            section class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                div class="flex items-center justify-between gap-2" {
                    h1 class="text-xl font-semibold" { "📋 Spec: " (data.symbol.as_str()) }
                    button
                        type="button"
                        class="px-3 py-2 rounded-md border border-surface-3/40 hover:bg-surface-3/20 text-sm"
                        data-copy-text=(copy_payload)
                        onclick="aetherCopyText(this)" {
                        "Copy Spec"
                    }
                }
                div class="text-xs text-text-muted" {
                    span class="badge badge-cyan" { (data.kind.as_str()) }
                    span class="ml-2 file-link text-blue-600 hover:underline cursor-pointer font-mono" data-path=(data.file.as_str()) { (data.file.as_str()) }
                }

                div class="space-y-3" {
                    (spec_block("Purpose", vec![data.spec.purpose.as_str()]))
                    (spec_block("Requirements", data.spec.requirements.iter().map(String::as_str).collect()))
                    (spec_block("Inputs", data.spec.inputs.iter().map(String::as_str).collect()))
                    (spec_block("Outputs", data.spec.outputs.iter().map(String::as_str).collect()))
                    (spec_block("Dependencies", data.spec.dependencies.iter().map(String::as_str).collect()))
                    (spec_block("Error Handling", data.spec.error_handling.iter().map(String::as_str).collect()))
                }
            }
        }
    })
}

fn spec_block(title: &str, values: Vec<&str>) -> maud::Markup {
    html! {
        div class="rounded-lg border border-surface-3/30 p-3 space-y-1" {
            h3 class="text-sm font-semibold" { (title) }
            ul class="list-disc pl-5 text-sm text-text-secondary space-y-1" {
                @for value in values {
                    li { (value) }
                }
            }
        }
    }
}

fn build_copy_payload(data: &crate::api::spec::SpecData) -> String {
    format!(
        "Symbol: {}\nKind: {}\nFile: {}\n\nPurpose:\n- {}\n\nRequirements:\n{}\n\nInputs:\n{}\n\nOutputs:\n{}\n\nDependencies:\n{}\n\nError Handling:\n{}",
        data.symbol,
        data.kind,
        data.file,
        data.spec.purpose,
        data.spec
            .requirements
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
        data.spec
            .inputs
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
        data.spec
            .outputs
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
        data.spec
            .dependencies
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
        data.spec
            .error_handling
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
