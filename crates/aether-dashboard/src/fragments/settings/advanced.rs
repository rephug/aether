use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    html! {
        form hx-post="/api/v1/settings/advanced"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Advanced Settings" }

            (helpers::collapsible_section("general-section", "General", true, render_general(config)))
            (helpers::collapsible_section("storage-section", "Storage", true, render_storage(config)))
            (helpers::collapsible_section("verification-section", "Verification", false, render_verification(config)))

            (helpers::save_reset_buttons("advanced"))
        }
    }
}

fn render_general(config: &AetherConfig) -> Markup {
    let c = &config.general;
    html! {
        (helpers::select_input(
            "general.log_level",
            "Log Level",
            &c.log_level,
            &[
                ("error", "Error"),
                ("warn", "Warn"),
                ("info", "Info"),
                ("debug", "Debug"),
                ("trace", "Trace"),
            ],
            "Log verbosity level",
        ))
    }
}

fn render_storage(config: &AetherConfig) -> Markup {
    let c = &config.storage;
    html! {
        (helpers::select_input(
            "storage.graph_backend",
            "Graph Backend",
            c.graph_backend.as_str(),
            &[
                ("surreal", "SurrealDB"),
                ("sqlite", "SQLite"),
                ("cozo", "Cozo (legacy)"),
            ],
            "Graph database backend. Restart required to change.",
        ))

        (helpers::toggle_input(
            "storage.mirror_sir_files",
            "Mirror SIR Files",
            c.mirror_sir_files,
            "Mirror SIR data to JSON files in .aether/sir/",
        ))
    }
}

fn render_verification(config: &AetherConfig) -> Markup {
    let c = &config.verify;
    html! {
        (helpers::select_input(
            "verify.mode",
            "Mode",
            c.mode.as_str(),
            &[
                ("host", "Host"),
                ("container", "Container"),
                ("microvm", "MicroVM"),
            ],
            "Where verification commands run",
        ))

        p class="text-xs text-text-muted italic pt-1" {
            "Verification command list is managed via config.toml directly."
        }
    }
}
