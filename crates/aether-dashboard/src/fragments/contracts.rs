use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::html;

use crate::support::{self, DashboardState};

pub(crate) async fn contracts_fragment(State(_state): State<Arc<DashboardState>>) -> Html<String> {
    support::html_markup_response(html! {
        div class="space-y-4" {
            (support::explanation_header(
                "Contract Health",
                "Shows rules you've set for how code should behave, and whether those rules are still being followed.",
                "Intent contracts per symbol with violation streaks and satisfaction rates. Violations are detected during SIR regeneration.",
                "Semantic contract monitoring: clause-level verification via embedding similarity and optional LLM judge. Streak threshold governs escalation from first_violation to active_violation.",
            ))

            div class="flex items-center justify-between gap-3" {
                h2 class="text-lg font-semibold" {
                    span class="beginner-only" { "Code Rules" }
                    span class="intermediate-only" { "Contract Health" }
                    span class="expert-only" { "Intent Contracts" }
                }
            }

            div id="contracts-health-container" class="min-h-[200px]" {}
        }
    })
}
