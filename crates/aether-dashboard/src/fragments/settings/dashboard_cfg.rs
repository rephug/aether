use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let c = &config.dashboard;
    html! {
        form hx-post="/api/v1/settings/dashboard"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Dashboard Settings" }

            (helpers::number_input(
                "port",
                "Port",
                c.port,
                "Dashboard HTTP server port. Restart required to change.",
                Some("1"),
                Some("65535"),
                None,
            ))

            (helpers::toggle_input(
                "enabled",
                "Enabled",
                c.enabled,
                "Enable the web dashboard server",
            ))

            (helpers::save_reset_buttons("dashboard"))
        }
    }
}
