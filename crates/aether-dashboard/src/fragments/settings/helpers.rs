use std::fmt::Display;

use maud::{Markup, PreEscaped, html};

/// Render a text input field with label and help text.
pub(crate) fn text_input(name: &str, label: &str, value: &str, help: &str) -> Markup {
    html! {
        div class="space-y-1" {
            label for=(name) class="block text-sm font-medium text-text-secondary" { (label) }
            input
                type="text"
                id=(name)
                name=(name)
                value=(value)
                class="w-full bg-surface-0/60 border border-surface-3/50 rounded-md px-3 py-2 text-sm text-text-primary focus:border-accent-cyan focus:ring-1 focus:ring-accent-cyan/50 focus:outline-none transition-colors";
            @if !help.is_empty() {
                p class="text-xs text-text-muted" { (help) }
            }
        }
    }
}

/// Render a number input field with optional min/max constraints.
pub(crate) fn number_input(
    name: &str,
    label: &str,
    value: impl Display,
    help: &str,
    min: Option<&str>,
    max: Option<&str>,
    step: Option<&str>,
) -> Markup {
    let val_str = value.to_string();
    html! {
        div class="space-y-1" {
            label for=(name) class="block text-sm font-medium text-text-secondary" { (label) }
            input
                type="number"
                id=(name)
                name=(name)
                value=(val_str)
                min=[min]
                max=[max]
                step=[step]
                class="w-full bg-surface-0/60 border border-surface-3/50 rounded-md px-3 py-2 text-sm text-text-primary focus:border-accent-cyan focus:ring-1 focus:ring-accent-cyan/50 focus:outline-none transition-colors";
            @if !help.is_empty() {
                p class="text-xs text-text-muted" { (help) }
            }
        }
    }
}

/// Render a dropdown select input.
pub(crate) fn select_input(
    name: &str,
    label: &str,
    current: &str,
    options: &[(&str, &str)],
    help: &str,
) -> Markup {
    html! {
        div class="space-y-1" {
            label for=(name) class="block text-sm font-medium text-text-secondary" { (label) }
            select
                id=(name)
                name=(name)
                class="w-full bg-surface-0/60 border border-surface-3/50 rounded-md px-3 py-2 text-sm text-text-primary focus:border-accent-cyan focus:ring-1 focus:ring-accent-cyan/50 focus:outline-none transition-colors"
            {
                @for (value, display) in options {
                    @if *value == current {
                        option value=(value) selected { (display) }
                    } @else {
                        option value=(value) { (display) }
                    }
                }
            }
            @if !help.is_empty() {
                p class="text-xs text-text-muted" { (help) }
            }
        }
    }
}

/// Render a dropdown with an HTMX trigger that reloads the section on change.
pub(crate) fn select_input_with_htmx(
    name: &str,
    label: &str,
    current: &str,
    options: &[(&str, &str)],
    help: &str,
    section: &str,
) -> Markup {
    let url = format!("/dashboard/frag/settings/{section}");
    html! {
        div class="space-y-1" {
            label for=(name) class="block text-sm font-medium text-text-secondary" { (label) }
            select
                id=(name)
                name=(name)
                hx-get=(url)
                hx-target="#settings-content"
                hx-trigger="change"
                hx-include="closest form"
                class="w-full bg-surface-0/60 border border-surface-3/50 rounded-md px-3 py-2 text-sm text-text-primary focus:border-accent-cyan focus:ring-1 focus:ring-accent-cyan/50 focus:outline-none transition-colors"
            {
                @for (value, display) in options {
                    @if *value == current {
                        option value=(value) selected { (display) }
                    } @else {
                        option value=(value) { (display) }
                    }
                }
            }
            @if !help.is_empty() {
                p class="text-xs text-text-muted" { (help) }
            }
        }
    }
}

/// Render a toggle/checkbox input.
pub(crate) fn toggle_input(name: &str, label: &str, checked: bool, help: &str) -> Markup {
    html! {
        div class="flex items-start gap-3 py-1" {
            div class="flex items-center h-6" {
                input
                    type="checkbox"
                    id=(name)
                    name=(name)
                    value="true"
                    checked[checked]
                    class="h-4 w-4 rounded border-surface-3/50 bg-surface-0/60 text-accent-cyan focus:ring-accent-cyan/50";
            }
            div class="space-y-0.5" {
                label for=(name) class="text-sm font-medium text-text-secondary cursor-pointer" { (label) }
                @if !help.is_empty() {
                    p class="text-xs text-text-muted" { (help) }
                }
            }
        }
    }
}

/// Render a slider input for float ranges.
pub(crate) fn slider_input(
    name: &str,
    label: &str,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    help: &str,
) -> Markup {
    let val_str = format!("{value:.2}");
    let min_str = format!("{min:.2}");
    let max_str = format!("{max:.2}");
    let step_str = format!("{step:.2}");
    let display_id = format!("{name}_display");
    // Inline JS to update the display value.
    let oninput = format!("document.getElementById('{display_id}').textContent=this.value");
    html! {
        div class="space-y-1" {
            div class="flex items-center justify-between" {
                label for=(name) class="text-sm font-medium text-text-secondary" { (label) }
                span id=(display_id) class="text-sm font-mono text-accent-cyan" { (val_str) }
            }
            input
                type="range"
                id=(name)
                name=(name)
                value=(val_str)
                min=(min_str)
                max=(max_str)
                step=(step_str)
                oninput=(oninput)
                class="w-full h-2 rounded-lg appearance-none cursor-pointer bg-surface-3/50 accent-accent-cyan";
            @if !help.is_empty() {
                p class="text-xs text-text-muted" { (help) }
            }
        }
    }
}

/// Render a read-only display field.
pub(crate) fn readonly_field(label: &str, value: &str, help: &str) -> Markup {
    html! {
        div class="space-y-1" {
            span class="block text-sm font-medium text-text-secondary" { (label) }
            div class="w-full bg-surface-0/30 border border-surface-3/30 rounded-md px-3 py-2 text-sm text-text-muted" {
                (value)
            }
            @if !help.is_empty() {
                p class="text-xs text-text-muted" { (help) }
            }
        }
    }
}

/// Render a section divider with a title.
pub(crate) fn section_divider(title: &str) -> Markup {
    html! {
        div class="pt-3 pb-1 border-b border-surface-3/30" {
            h4 class="text-xs font-semibold uppercase tracking-wider text-text-muted" { (title) }
        }
    }
}

/// Render Save and Reset to Defaults buttons.
pub(crate) fn save_reset_buttons(section: &str) -> Markup {
    let reset_url = format!("/api/v1/settings/{section}/reset");
    html! {
        div class="flex items-center gap-3 pt-4 border-t border-surface-3/30" {
            button
                type="submit"
                class="px-4 py-2 rounded-md bg-accent-cyan/20 text-accent-cyan text-sm font-medium hover:bg-accent-cyan/30 transition-colors border border-accent-cyan/30"
            {
                "Save"
            }
            button
                type="button"
                hx-post=(reset_url)
                hx-target="#settings-content"
                hx-swap="innerHTML"
                class="px-4 py-2 rounded-md bg-surface-3/20 text-text-secondary text-sm font-medium hover:bg-surface-3/30 transition-colors border border-surface-3/30"
            {
                "Reset to Defaults"
            }
            div id="settings-status" class="ml-auto" {}
        }
    }
}

/// Render a success banner.
pub(crate) fn success_banner(message: &str) -> Markup {
    html! {
        div class="rounded-md bg-accent-green/10 border border-accent-green/30 px-4 py-3 text-sm text-accent-green flex items-center gap-2" {
            svg class="w-4 h-4 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" {
                path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" {}
            }
            span { (message) }
        }
    }
}

/// Render an error banner with a list of validation errors.
pub(crate) fn error_banner(errors: &[String]) -> Markup {
    html! {
        div class="rounded-md bg-accent-red/10 border border-accent-red/30 px-4 py-3 text-sm text-accent-red" {
            p class="font-medium" { "Validation errors:" }
            ul class="list-disc list-inside mt-1 space-y-0.5" {
                @for err in errors {
                    li { (err) }
                }
            }
        }
    }
}

/// Render a "restart required" banner.
pub(crate) fn restart_required_banner(setting_name: &str) -> Markup {
    html! {
        div class="rounded-md bg-accent-orange/10 border border-accent-orange/30 px-4 py-3 text-sm text-accent-orange" {
            p {
                "Restart AETHER to apply changes to "
                strong { (setting_name) }
                "."
            }
            p class="mt-2 text-xs text-text-muted" {
                (PreEscaped(r#"<script>
                    (function(){
                        var btn = document.getElementById('restart-btn');
                        if (btn && window.__TAURI__) {
                            btn.style.display = 'inline-block';
                            btn.onclick = function() { window.__TAURI__.core.invoke('restart_app'); };
                        }
                    })();
                </script>"#))
                button
                    id="restart-btn"
                    style="display:none"
                    class="mt-1 px-3 py-1 rounded-md bg-accent-orange/20 text-accent-orange text-xs font-medium hover:bg-accent-orange/30 border border-accent-orange/30"
                {
                    "Restart Now"
                }
                span class="restart-manual-hint" {
                    "Restart aetherd manually to apply."
                }
            }
        }
    }
}

/// Render a collapsible subsection.
pub(crate) fn collapsible_section(_id: &str, title: &str, open: bool, content: Markup) -> Markup {
    html! {
        details open[open] class="rounded-lg border border-surface-3/30 bg-surface-0/20" {
            summary class="cursor-pointer px-4 py-2.5 text-sm font-medium text-text-secondary hover:text-text-primary transition-colors select-none" {
                (title)
            }
            div class="px-4 pb-4 space-y-4" {
                (content)
            }
        }
    }
}
