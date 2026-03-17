use std::sync::{Arc, Mutex};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime, image::Image};

/// Tray icon states map to tooltip text and menu labels.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TrayState {
    Idle,
    Indexing,
    Error(String),
}

#[allow(dead_code)]
impl TrayState {
    pub fn tooltip(&self, workspace: &str, symbol_count: usize) -> String {
        let base = format!("AETHER \u{2014} {workspace} \u{2014} {symbol_count} symbols");
        match self {
            TrayState::Idle => base,
            TrayState::Indexing => format!("{base} (indexing\u{2026})"),
            TrayState::Error(msg) => format!("{base} \u{2014} Error: {msg}"),
        }
    }
}

#[allow(dead_code)]
pub struct TrayManager {
    pub state: Arc<Mutex<TrayState>>,
}

#[allow(dead_code)]
impl TrayManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TrayState::Idle)),
        }
    }

    pub fn set_state(&self, new_state: TrayState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = new_state;
        }
    }

    pub fn current_state(&self) -> TrayState {
        self.state
            .lock()
            .map(|g| g.clone())
            .unwrap_or(TrayState::Idle)
    }
}

pub fn build_tray<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    let open_item = MenuItem::with_id(app, "open", "Open Dashboard", true, None::<&str>)?;
    let pause_item = MenuItem::with_id(app, "pause", "Pause Indexing", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&open_item, &pause_item, &quit_item])?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(
            app.default_window_icon()
                .cloned()
                .unwrap_or_else(|| Image::new_owned(vec![0u8; 4], 1, 1)),
        )
        .tooltip("AETHER")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "pause" => {
                // Toggle pause state via app state
                if let Some(pause_flag) = app.try_state::<crate::PauseFlag>() {
                    let was_paused = pause_flag
                        .0
                        .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                    tracing::info!(paused = !was_paused, "indexer pause toggled");
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}
