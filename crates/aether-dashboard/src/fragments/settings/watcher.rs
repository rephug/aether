use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let watcher = match config.watcher.as_ref() {
        Some(w) => w,
        None => {
            return html! {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                    h3 class="text-sm font-semibold text-text-primary pb-2" { "Watcher Settings" }
                    p class="text-sm text-text-muted" {
                        "Watcher is not configured. Add a "
                        code class="text-xs bg-surface-3/30 px-1 py-0.5 rounded" { "[watcher]" }
                        " section to your config.toml to enable file watching."
                    }
                }
            };
        }
    };

    html! {
        form hx-post="/api/v1/settings/watcher"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Watcher Settings" }

            (helpers::text_input(
                "realtime_model",
                "Realtime Model",
                &watcher.realtime_model,
                "Model for realtime file change inference",
            ))

            (helpers::text_input(
                "realtime_provider",
                "Realtime Provider",
                &watcher.realtime_provider,
                "Provider for realtime inference",
            ))

            (helpers::section_divider("Git Triggers"))

            (helpers::toggle_input(
                "trigger_on_branch_switch",
                "Trigger on Branch Switch",
                watcher.trigger_on_branch_switch,
                "Re-index when switching git branches",
            ))

            (helpers::toggle_input(
                "trigger_on_git_pull",
                "Trigger on Git Pull",
                watcher.trigger_on_git_pull,
                "Re-index after git pull",
            ))

            (helpers::toggle_input(
                "trigger_on_merge",
                "Trigger on Merge",
                watcher.trigger_on_merge,
                "Re-index after git merge",
            ))

            (helpers::toggle_input(
                "trigger_on_build_success",
                "Trigger on Build Success",
                watcher.trigger_on_build_success,
                "Re-index after successful build",
            ))

            (helpers::toggle_input(
                "git_trigger_changed_files_only",
                "Changed Files Only",
                watcher.git_trigger_changed_files_only,
                "Only re-index files changed by git operation",
            ))

            (helpers::section_divider("Debounce"))

            (helpers::number_input(
                "git_debounce_secs",
                "Git Debounce (seconds)",
                watcher.git_debounce_secs,
                "Seconds to wait after git events before processing",
                Some("0"),
                None,
                Some("0.1"),
            ))

            (helpers::save_reset_buttons("watcher"))
        }
    }
}
