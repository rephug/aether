// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod notifications;
mod tray;
mod updater;
pub mod wizard;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use aether_config::load_workspace_config;
use aether_store::SqliteStore;
use tauri::Manager;

/// Shared pause flag — the indexer thread checks this before processing events.
pub struct PauseFlag(pub Arc<AtomicBool>);

impl PauseFlag {
    pub fn from(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }
}

/// Application state shared with Tauri commands.
pub struct AppState {
    pub workspace: PathBuf,
    pub dashboard_port: u16,
    store: Option<Arc<SqliteStore>>,
}

impl AppState {
    pub fn symbol_count(&self) -> usize {
        self.store
            .as_ref()
            .and_then(|s| s.count_symbols_with_sir().ok())
            .map(|(total, _with_sir)| total)
            .unwrap_or(0)
    }
}

fn main() {
    // Initialize tracing for desktop logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Determine startup mode:
    // 1. CLI arg overrides everything
    // 2. Else check last-used workspace from app data dir
    // 3. If workspace has .aether/config.toml → normal mode
    // 4. Otherwise → wizard mode
    let cli_workspace = std::env::args().nth(1).map(PathBuf::from);
    let last_workspace = wizard::load_last_workspace_pre_tauri().map(PathBuf::from);

    let resolved_workspace = cli_workspace.or(last_workspace);

    let is_wizard_mode = match &resolved_workspace {
        Some(ws) => {
            // Workspace path exists but has no config → wizard mode
            !aether_config::config_path(ws).exists()
        }
        None => true, // No workspace at all → wizard mode
    };

    if is_wizard_mode {
        tracing::info!("no configured workspace found — starting in wizard mode");
        start_wizard_mode(resolved_workspace);
    } else {
        let workspace = resolved_workspace.expect("resolved above");
        let workspace = match workspace.canonicalize() {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(error = %err, path = %workspace.display(),
                    "failed to resolve workspace, falling back to wizard");
                start_wizard_mode(None);
                return;
            }
        };
        tracing::info!(workspace = %workspace.display(), "starting AETHER Desktop (normal mode)");
        start_normal_mode(workspace);
    }
}

// ---------------------------------------------------------------------------
// Normal mode — full dashboard + indexer
// ---------------------------------------------------------------------------

fn start_normal_mode(workspace: PathBuf) {
    // Load config (returns defaults if no config.toml — safe).
    let config = match load_workspace_config(&workspace) {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(error = %err, "failed to load workspace config");
            std::process::exit(1);
        }
    };

    // Open a read-only store for symbol counts in Tauri commands.
    let store = match SqliteStore::open_readonly(&workspace) {
        Ok(s) => Some(Arc::new(s)),
        Err(err) => {
            tracing::warn!(error = %err, "failed to open SQLite store (fresh workspace?)");
            None
        }
    };

    // Start dashboard HTTP server on ephemeral port.
    let dashboard_port = start_dashboard_server(&workspace);

    // Check if this is a fresh workspace (just created by wizard).
    let is_fresh = store
        .as_ref()
        .map(|s| {
            s.count_symbols_with_sir()
                .map(|(total, _)| total < 10)
                .unwrap_or(true)
        })
        .unwrap_or(true);

    // Build shared app state.
    let app_state = AppState {
        workspace: workspace.clone(),
        dashboard_port,
        store,
    };

    // Create shared pause flag for indexer control.
    let pause_flag = Arc::new(AtomicBool::new(false));

    // Start indexer on a background thread.
    let indexer_workspace = workspace.clone();
    let sir_concurrency = config.inference.concurrency.max(1);
    let indexer_pause = pause_flag.clone();
    std::thread::spawn(move || {
        let indexer_config = aetherd::indexer::IndexerConfig {
            workspace: indexer_workspace,
            debounce_ms: 500,
            print_events: false,
            print_sir: false,
            embeddings_only: false,
            force: false,
            full: false,
            deep: false,
            turbo_concurrency: None,
            dry_run: false,
            sir_concurrency,
            lifecycle_logs: false,
            inference_provider: None,
            inference_model: None,
            inference_endpoint: None,
            inference_api_key_env: None,
            pause_flag: Some(indexer_pause),
        };
        if let Err(err) = aetherd::indexer::run_indexing_loop(indexer_config) {
            tracing::error!(error = %err, "indexer loop exited with error");
        }
    });

    // Launch Tauri app.
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .manage(PauseFlag::from(pause_flag))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_workspace_path,
            commands::pause_indexing,
            commands::resume_indexing,
            commands::restart_app,
            commands::check_for_update,
            commands::install_update,
            commands::get_update_preferences,
            commands::set_update_preferences,
            // Wizard commands (for "Run Setup Again")
            wizard::pick_directory,
            wizard::detect_environment,
            wizard::estimate_index_time,
            wizard::finalize_wizard,
            wizard::get_last_workspace,
            wizard::save_last_workspace,
            wizard::is_first_run,
        ])
        .setup(move |app| {
            // Register updater plugin.
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Build system tray.
            tray::build_tray(app.handle())?;

            // Save this workspace as last-used.
            let _ =
                wizard::save_last_workspace(app.handle().clone(), workspace.display().to_string());

            // Navigate the webview to the internal dashboard server.
            if let Some(window) = app.get_webview_window("main") {
                let page = if is_fresh { "first-run-progress" } else { "" };
                let url: tauri::Url = format!("http://127.0.0.1:{dashboard_port}/dashboard/{page}")
                    .parse()
                    .expect("valid dashboard URL");
                window.navigate(url)?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Minimize to tray on close instead of quitting.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running AETHER Desktop");
}

// ---------------------------------------------------------------------------
// Wizard mode — stateless wizard UI, no indexer
// ---------------------------------------------------------------------------

fn start_wizard_mode(workspace: Option<PathBuf>) {
    let wizard_port = start_wizard_server();

    // Provide a minimal AppState for commands that require it.
    let app_state = AppState {
        workspace: workspace
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))),
        dashboard_port: wizard_port,
        store: None,
    };

    let pause_flag = Arc::new(AtomicBool::new(false));

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .manage(PauseFlag::from(pause_flag))
        .invoke_handler(tauri::generate_handler![
            // Only register wizard + basic commands in wizard mode
            commands::restart_app,
            commands::get_workspace_path,
            wizard::pick_directory,
            wizard::detect_environment,
            wizard::estimate_index_time,
            wizard::finalize_wizard,
            wizard::get_last_workspace,
            wizard::save_last_workspace,
            wizard::is_first_run,
        ])
        .setup(move |app| {
            // Build system tray (even in wizard mode for consistency).
            tray::build_tray(app.handle())?;

            // Navigate to the wizard UI.
            if let Some(window) = app.get_webview_window("main") {
                let url: tauri::Url = format!("http://127.0.0.1:{wizard_port}/dashboard/")
                    .parse()
                    .expect("valid wizard URL");
                window.navigate(url)?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running AETHER Desktop (wizard mode)");
}

// ---------------------------------------------------------------------------
// Server starters
// ---------------------------------------------------------------------------

/// Start the full dashboard HTTP server on an ephemeral port.
fn start_dashboard_server(workspace: &std::path::Path) -> u16 {
    let ws = workspace.to_path_buf();

    let (tx, rx) = std::sync::mpsc::channel::<u16>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build dashboard tokio runtime");

        rt.block_on(async move {
            let state = match aether_dashboard::SharedState::open_readonly_async(&ws).await {
                Ok(s) => std::sync::Arc::new(s),
                Err(err) => {
                    tracing::error!(error = %err, "failed to open dashboard state");
                    let _ = tx.send(0);
                    return;
                }
            };

            let router = aether_dashboard::dashboard_router(state);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind dashboard to ephemeral port");
            let port = listener.local_addr().unwrap().port();
            tracing::info!(port = port, "dashboard server listening");
            let _ = tx.send(port);

            if let Err(err) = axum::serve(listener, router.into_make_service()).await {
                tracing::error!(error = %err, "dashboard server error");
            }
        });
    });

    rx.recv().expect("failed to receive dashboard port")
}

/// Start a stateless wizard-only HTTP server on an ephemeral port.
fn start_wizard_server() -> u16 {
    let (tx, rx) = std::sync::mpsc::channel::<u16>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("failed to build wizard tokio runtime");

        rt.block_on(async move {
            let router = aether_dashboard::wizard_router();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("failed to bind wizard server to ephemeral port");
            let port = listener.local_addr().unwrap().port();
            tracing::info!(port = port, "wizard server listening");
            let _ = tx.send(port);

            if let Err(err) = axum::serve(listener, router.into_make_service()).await {
                tracing::error!(error = %err, "wizard server error");
            }
        });
    });

    rx.recv().expect("failed to receive wizard port")
}
