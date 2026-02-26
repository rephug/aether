use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use maud::{Markup, html};

use crate::support::{self, DashboardState};

pub(crate) async fn graph_fragment(State(_state): State<Arc<DashboardState>>) -> Html<String> {
    support::html_markup_response(placeholder_page(
        "Dependencies",
        "Graph visualization arrives in Phase 7.6b",
        Some("aether index"),
    ))
}

pub(crate) async fn drift_fragment(State(_state): State<Arc<DashboardState>>) -> Html<String> {
    support::html_markup_response(placeholder_page(
        "Drift Report",
        "Drift table and scatter plot arrive in Phase 7.6b",
        Some("aether drift-report"),
    ))
}

pub(crate) async fn coupling_fragment(State(_state): State<Arc<DashboardState>>) -> Html<String> {
    support::html_markup_response(placeholder_page(
        "Coupling Map",
        "Coupling heatmap arrives in Phase 7.6b",
        Some("aether mine-coupling"),
    ))
}

pub(crate) async fn health_fragment(State(_state): State<Arc<DashboardState>>) -> Html<String> {
    support::html_markup_response(placeholder_page(
        "Health",
        "Health visualization arrives in Phase 7.6b",
        Some("aether index"),
    ))
}

fn placeholder_page(title: &str, msg: &str, cmd: Option<&str>) -> Markup {
    html! {
        div class="space-y-4" {
            h2 class="text-lg font-semibold" { (title) }
            (support::html_empty_state(msg, cmd))
        }
    }
}
