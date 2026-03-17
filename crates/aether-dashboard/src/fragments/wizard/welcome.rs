use maud::{Markup, html};

/// Step 1: Welcome page.
pub(crate) fn render() -> Markup {
    html! {
        div class="text-center space-y-6 py-4" {
            div class="text-4xl" { "\u{2728}" }

            h2 class="text-xl font-semibold text-text-primary dark:text-slate-100" {
                "Welcome to AETHER"
            }

            p class="text-text-secondary dark:text-slate-300 max-w-md mx-auto leading-relaxed" {
                "AETHER understands your codebase \u{2014} and documents. "
                "It indexes every symbol, tracks relationships, and surfaces intelligence "
                "to help you navigate with confidence."
            }

            p class="text-sm text-text-muted dark:text-slate-400" {
                "Let\u{2019}s get you set up in a few quick steps."
            }

            div class="pt-4" {
                button
                    class="px-6 py-2.5 bg-accent-cyan text-white rounded-lg hover:bg-accent-cyan/90 transition-colors font-medium"
                    hx-get="/dashboard/frag/wizard/step/2"
                    hx-target="#wizard-content"
                {
                    "Get Started \u{2192}"
                }
            }
        }
    }
}
