mod confirm;
mod detect;
mod environment;
pub(crate) mod progress;
mod provider;
mod welcome;
mod workspace;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::routing::get;
use maud::{Markup, html};

use crate::support::DashboardState;

/// Wizard fragment router — stateless, no `DashboardState` required.
/// Used by the wizard-only server in wizard mode.
pub fn wizard_fragment_router() -> Router {
    Router::new()
        .route("/dashboard/frag/wizard/step/{n}", get(wizard_step_fragment))
        .route(
            "/dashboard/frag/wizard/detect",
            get(detect::detect_fragment),
        )
}

/// Wizard step handler for the main dashboard router (accepts + ignores DashboardState).
/// Used by "Run Setup Again" in the sidebar.
pub(crate) async fn wizard_step_with_state(
    State(_state): State<Arc<DashboardState>>,
    Path(step): Path<u32>,
    Query(params): Query<HashMap<String, String>>,
) -> Html<String> {
    wizard_step_fragment(Path(step), Query(params)).await
}

/// Wizard detect handler for the main dashboard router (accepts + ignores DashboardState).
pub(crate) async fn wizard_detect_with_state(
    State(_state): State<Arc<DashboardState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Html<String> {
    detect::detect_fragment(Query(params)).await
}

/// Dispatch to the correct step fragment based on the step number.
async fn wizard_step_fragment(
    Path(step): Path<u32>,
    Query(params): Query<HashMap<String, String>>,
) -> Html<String> {
    let markup = match step {
        1 => welcome::render(),
        2 => workspace::render(&params),
        3 => environment::render(&params),
        4 => provider::render(&params),
        5 => confirm::render(&params),
        _ => welcome::render(),
    };
    Html(wizard_card(step, markup).into_string())
}

/// Wrap a step's content in the wizard card with step indicator.
fn wizard_card(current_step: u32, content: Markup) -> Markup {
    let steps = ["Welcome", "Workspace", "Environment", "Provider", "Confirm"];

    html! {
        div id="wizard-content" class="bg-surface-1 dark:bg-slate-800 rounded-xl border border-surface-3/50 dark:border-slate-700 shadow-lg overflow-hidden" {
            // Step indicator
            div class="px-6 pt-5 pb-3 border-b border-surface-3/30 dark:border-slate-700" {
                div class="flex items-center justify-between text-xs text-text-muted dark:text-slate-400 mb-3" {
                    span { "AETHER Setup" }
                    span { "Step " (current_step) " of 5" }
                }
                div class="flex gap-1.5" {
                    @for (i, label) in steps.iter().enumerate() {
                        @let step_num = (i as u32) + 1;
                        div class="flex-1" {
                            div class={
                                "h-1.5 rounded-full transition-colors "
                                @if step_num <= current_step { "bg-accent-cyan" }
                                @else { "bg-surface-3/50 dark:bg-slate-600" }
                            } {}
                            div class={
                                "text-[10px] mt-1 text-center "
                                @if step_num == current_step { "text-accent-cyan font-medium" }
                                @else { "text-text-muted dark:text-slate-500" }
                            } { (label) }
                        }
                    }
                }
            }

            // Step content
            div class="p-6" {
                (content)
            }
        }
    }
}

/// Shared "Back" button markup.
pub(crate) fn back_button(step: u32, params: &HashMap<String, String>) -> Markup {
    let workspace = params.get("workspace_path").cloned().unwrap_or_default();
    let provider = params.get("provider").cloned().unwrap_or_default();
    html! {
        button
            class="px-4 py-2 text-sm text-text-secondary dark:text-slate-300 hover:bg-surface-2 dark:hover:bg-slate-700 rounded-lg transition-colors"
            hx-get={ "/dashboard/frag/wizard/step/" (step) }
            hx-target="#wizard-content"
            hx-include="[name='workspace_path'], [name='provider'], [name='enable_batch'], [name='enable_continuous']"
        {
            "\u{2190} Back"
        }
        // Carry forward hidden fields
        input type="hidden" name="workspace_path" value=(workspace) {}
        input type="hidden" name="provider" value=(provider) {}
    }
}

/// Shared "Next" button markup.
pub(crate) fn next_button(step: u32) -> Markup {
    html! {
        button
            class="px-5 py-2 text-sm bg-accent-cyan text-white rounded-lg hover:bg-accent-cyan/90 transition-colors font-medium"
            hx-get={ "/dashboard/frag/wizard/step/" (step) }
            hx-target="#wizard-content"
            hx-include="[name='workspace_path'], [name='provider'], [name='enable_batch'], [name='enable_continuous'], [name='api_key_env']"
        {
            "Next \u{2192}"
        }
    }
}
