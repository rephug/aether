// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod notifications;
mod tray;

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
    store: Arc<SqliteStore>,
}

impl AppState {
    pub fn symbol_count(&self) -> usize {
        self.store
            .count_symbols_with_sir()
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

    // Resolve workspace from first CLI arg or current directory.
    let workspace = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("failed to get current directory"));

    let workspace = match workspace.canonicalize() {
        Ok(p) => p,
        Err(err) => {
            tracing::error!(error = %err, path = %workspace.display(), "failed to resolve workspace");
            std::process::exit(1);
        }
    };

    tracing::info!(workspace = %workspace.display(), "starting AETHER Desktop");

    // Load config.
    let config = match load_workspace_config(&workspace) {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(error = %err, "failed to load workspace config");
            std::process::exit(1);
        }
    };

    // Open a read-only store for symbol counts in Tauri commands.
    let store = match SqliteStore::open_readonly(&workspace) {
        Ok(s) => Arc::new(s),
        Err(err) => {
            tracing::error!(error = %err, "failed to open SQLite store");
            std::process::exit(1);
        }
    };

    // Start dashboard HTTP server on ephemeral port.
    let dashboard_port = start_dashboard_server(&workspace);

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
        .manage(app_state)
        .manage(PauseFlag::from(pause_flag))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_workspace_path,
            commands::pause_indexing,
            commands::resume_indexing,
            commands::restart_app,
        ])
        .setup(move |app| {
            // Build system tray.
            tray::build_tray(app.handle())?;

            // Navigate the webview to the internal dashboard server.
            if let Some(window) = app.get_webview_window("main") {
                let url: tauri::Url = format!("http://127.0.0.1:{dashboard_port}/dashboard/")
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

/// Start the dashboard HTTP server on an ephemeral port and return the actual port.
fn start_dashboard_server(workspace: &std::path::Path) -> u16 {
    let ws = workspace.to_path_buf();

    // Use a channel to communicate the bound port back to the main thread.
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
