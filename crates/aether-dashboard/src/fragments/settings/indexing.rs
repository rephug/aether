use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig, workspace: &std::path::Path) -> Markup {
    html! {
        form hx-post="/api/v1/settings/indexing"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Indexing Settings" }

            (helpers::readonly_field(
                "Workspace Path",
                &workspace.display().to_string(),
                "Current workspace root. Restart required to change.",
            ))

            @if let Some(w) = &config.watcher {
                (helpers::section_divider("Watcher Settings"))

                (helpers::number_input(
                    "git_debounce_secs",
                    "Git Debounce (seconds)",
                    w.git_debounce_secs,
                    "Seconds to wait after file changes before processing",
                    None,
                    None,
                    Some("0.1"),
                ))

                p class="text-xs text-text-muted italic pt-1" {
                    "See the Watcher tab for full watcher configuration."
                }
            } @else {
                (helpers::section_divider("Watcher Settings"))

                div class="rounded-md bg-surface-0/30 border border-surface-3/30 px-4 py-3 text-sm text-text-muted" {
                    "Watcher is not configured. Add a "
                    code class="text-accent-cyan" { "[watcher]" }
                    " section to config.toml to enable."
                }
            }

            (helpers::save_reset_buttons("indexing"))
        }
    }
}
