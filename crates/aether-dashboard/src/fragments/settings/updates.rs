use maud::{Markup, PreEscaped, html};

use super::helpers;

/// Render the Updates settings section.
///
/// This section is desktop-only: when running in the Tauri shell, it shows
/// update preferences and a "Check Now" button. When running in a browser,
/// it shows an informational message.
pub(crate) fn render() -> Markup {
    html! {
        div id="updates-section" {
            // Desktop-only content (hidden by default, shown via JS when __TAURI__ exists)
            div id="updates-desktop" style="display:none" {
                form
                    id="updates-form"
                    class="space-y-4"
                {
                    (helpers::section_divider("Auto-Update"))

                    (helpers::toggle_input(
                        "auto_check",
                        "Auto-check for updates",
                        true,
                        "Check for new versions automatically",
                    ))

                    (helpers::select_input(
                        "frequency",
                        "Check frequency",
                        "launch",
                        &[("launch", "On launch"), ("daily", "Daily"), ("weekly", "Weekly")],
                        "How often to check for updates",
                    ))

                    (helpers::toggle_input(
                        "include_prerelease",
                        "Include pre-release versions",
                        false,
                        "Receive beta and release candidate updates",
                    ))

                    (helpers::section_divider("Status"))

                    (helpers::readonly_field(
                        "Current version",
                        env!("CARGO_PKG_VERSION"),
                        "",
                    ))

                    div id="update-check-result" class="text-sm text-text-muted" {}

                    div class="flex items-center gap-3 pt-4 border-t border-surface-3/30" {
                        button
                            type="button"
                            id="check-now-btn"
                            class="px-4 py-2 rounded-md bg-accent-cyan/20 text-accent-cyan text-sm font-medium hover:bg-accent-cyan/30 transition-colors border border-accent-cyan/30"
                        {
                            "Check Now"
                        }
                        button
                            type="button"
                            id="save-update-prefs-btn"
                            class="px-4 py-2 rounded-md bg-surface-3/20 text-text-secondary text-sm font-medium hover:bg-surface-3/30 transition-colors border border-surface-3/30"
                        {
                            "Save Preferences"
                        }
                    }
                }
            }

            // Browser fallback message
            div id="updates-browser" {
                div class="rounded-md bg-surface-3/20 border border-surface-3/40 px-4 py-8 text-center" {
                    p class="text-sm text-text-muted" {
                        "Auto-update is only available in the AETHER Desktop app."
                    }
                    p class="text-xs text-text-muted mt-2" {
                        "Download the desktop app from "
                        span class="text-accent-cyan" { "GitHub Releases" }
                        " for automatic updates."
                    }
                }
            }

            // Inline script for Tauri detection and update commands.
            // Uses textContent (not innerHTML) to avoid XSS from release notes.
            (PreEscaped(r#"<script>
            (function() {
                if (window.__TAURI__) {
                    document.getElementById('updates-desktop').style.display = 'block';
                    document.getElementById('updates-browser').style.display = 'none';

                    // Load saved preferences
                    window.__TAURI__.core.invoke('get_update_preferences').then(function(prefs) {
                        if (prefs) {
                            var autoCheck = document.getElementById('auto_check');
                            var freq = document.getElementById('frequency');
                            var prerelease = document.getElementById('include_prerelease');
                            if (autoCheck) autoCheck.checked = prefs.auto_check;
                            if (freq) freq.value = prefs.frequency;
                            if (prerelease) prerelease.checked = prefs.include_prerelease;
                        }
                    });

                    // Check Now button
                    document.getElementById('check-now-btn').onclick = function() {
                        var result = document.getElementById('update-check-result');
                        result.textContent = 'Checking for updates\u2026';
                        result.className = 'text-sm text-text-muted';
                        window.__TAURI__.core.invoke('check_for_update').then(function(info) {
                            if (info) {
                                result.textContent = '';
                                var ver = document.createElement('span');
                                ver.className = 'text-accent-green font-medium';
                                ver.textContent = 'Update available: v' + info.version;
                                result.appendChild(ver);
                                if (info.body) {
                                    var br = document.createElement('br');
                                    result.appendChild(br);
                                    var notes = document.createElement('span');
                                    notes.className = 'text-xs text-text-muted';
                                    notes.textContent = info.body.substring(0, 200);
                                    result.appendChild(notes);
                                }
                                // Install Update button
                                var installBr = document.createElement('br');
                                result.appendChild(installBr);
                                var installBtn = document.createElement('button');
                                installBtn.type = 'button';
                                installBtn.className = 'mt-2 px-4 py-2 rounded-md bg-accent-cyan/20 text-accent-cyan text-sm font-medium hover:bg-accent-cyan/30 transition-colors border border-accent-cyan/30';
                                installBtn.textContent = 'Install Update';
                                installBtn.onclick = function() {
                                    installBtn.disabled = true;
                                    installBtn.textContent = 'Installing\u2026';
                                    installBtn.className = 'mt-2 px-4 py-2 rounded-md bg-surface-3/20 text-text-muted text-sm font-medium border border-surface-3/30 cursor-not-allowed';
                                    window.__TAURI__.core.invoke('install_update').then(function() {
                                        installBtn.textContent = 'Update installed \u2014 restart to apply';
                                        installBtn.className = 'mt-2 px-4 py-2 rounded-md bg-accent-green/20 text-accent-green text-sm font-medium border border-accent-green/30';
                                    }).catch(function(err) {
                                        installBtn.disabled = false;
                                        installBtn.textContent = 'Install Update';
                                        installBtn.className = 'mt-2 px-4 py-2 rounded-md bg-accent-cyan/20 text-accent-cyan text-sm font-medium hover:bg-accent-cyan/30 transition-colors border border-accent-cyan/30';
                                        var errMsg = document.createElement('span');
                                        errMsg.className = 'ml-2 text-xs text-accent-red';
                                        errMsg.textContent = 'Install failed: ' + err;
                                        installBtn.parentNode.appendChild(errMsg);
                                    });
                                };
                                result.appendChild(installBtn);
                            } else {
                                result.textContent = '';
                                var ok = document.createElement('span');
                                ok.className = 'text-accent-green';
                                ok.textContent = 'You are up to date.';
                                result.appendChild(ok);
                            }
                        }).catch(function(err) {
                            result.textContent = '';
                            var errSpan = document.createElement('span');
                            errSpan.className = 'text-accent-red';
                            errSpan.textContent = 'Check failed: ' + err;
                            result.appendChild(errSpan);
                        });
                    };

                    // Save preferences button
                    document.getElementById('save-update-prefs-btn').onclick = function() {
                        var prefs = {
                            auto_check: document.getElementById('auto_check').checked,
                            frequency: document.getElementById('frequency').value,
                            include_prerelease: document.getElementById('include_prerelease').checked
                        };
                        window.__TAURI__.core.invoke('set_update_preferences', { prefs: prefs }).then(function() {
                            var result = document.getElementById('update-check-result');
                            result.textContent = '';
                            var ok = document.createElement('span');
                            ok.className = 'text-accent-green';
                            ok.textContent = 'Preferences saved.';
                            result.appendChild(ok);
                        }).catch(function(err) {
                            var result = document.getElementById('update-check-result');
                            result.textContent = '';
                            var errSpan = document.createElement('span');
                            errSpan.className = 'text-accent-red';
                            errSpan.textContent = 'Save failed: ' + err;
                            result.appendChild(errSpan);
                        });
                    };
                }
            })();
            </script>"#))
        }
    }
}
