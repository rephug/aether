use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use aether_config::InferenceProviderKind;
use aether_infer::ProviderOverrides;
use aether_store::SqliteStore;
use anyhow::{Context, Result};
use notify::{Config, Event, PollWatcher, RecursiveMode, Watcher};

use crate::observer::{DebounceQueue, ObserverState, is_ignored_path};
use crate::sir_pipeline::SirPipeline;

#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub workspace: PathBuf,
    pub debounce_ms: u64,
    pub print_events: bool,
    pub print_sir: bool,
    pub sir_concurrency: usize,
    pub lifecycle_logs: bool,
    pub inference_provider: Option<InferenceProviderKind>,
    pub inference_model: Option<String>,
    pub inference_endpoint: Option<String>,
    pub inference_api_key_env: Option<String>,
}

pub fn run_initial_index_once(config: &IndexerConfig) -> Result<()> {
    let _ = initialize_indexer(config)?;
    Ok(())
}

pub fn run_indexing_loop(config: IndexerConfig) -> Result<()> {
    let (mut observer, store, sir_pipeline) = initialize_indexer(&config)?;

    if config.lifecycle_logs {
        println!("INDEX: watching");
    }

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = PollWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        Config::default().with_poll_interval(Duration::from_millis(200)),
    )
    .context("failed to initialize file watcher")?;

    watcher
        .watch(&config.workspace, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch workspace {}", config.workspace.display()))?;

    let debounce_window = Duration::from_millis(config.debounce_ms);
    let poll_interval = Duration::from_millis(50);
    let mut debounce_queue = DebounceQueue::default();
    let mut stdout = std::io::stdout();

    loop {
        match rx.recv_timeout(poll_interval) {
            Ok(result) => {
                if let Err(err) =
                    enqueue_event_paths(&config.workspace, result, &mut debounce_queue)
                {
                    tracing::warn!(error = ?err, "watch event error");
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("watcher channel disconnected"));
            }
        }

        while let Ok(result) = rx.try_recv() {
            if let Err(err) = enqueue_event_paths(&config.workspace, result, &mut debounce_queue) {
                tracing::warn!(error = ?err, "watch event error");
            }
        }

        for path in debounce_queue.drain_due(Instant::now(), debounce_window) {
            match observer.process_path(&path) {
                Ok(Some(event)) => {
                    if config.print_events {
                        let line = serde_json::to_string(&event)
                            .context("failed to serialize symbol-change event")?;
                        println!("{line}");
                    }

                    if let Err(err) =
                        sir_pipeline.process_event(&store, &event, config.print_sir, &mut stdout)
                    {
                        tracing::error!(
                            file_path = %event.file_path,
                            error = %err,
                            "SIR pipeline error"
                        );
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::error!(
                    path = %path.display(),
                    error = %err,
                    "process error"
                ),
            }
        }
    }
}

fn initialize_indexer(config: &IndexerConfig) -> Result<(ObserverState, SqliteStore, SirPipeline)> {
    if config.lifecycle_logs {
        println!("INDEX: starting");
    }

    let mut observer = ObserverState::new(config.workspace.clone())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(&config.workspace).context("failed to initialize local store")?;
    let sir_pipeline = SirPipeline::new(
        config.workspace.clone(),
        config.sir_concurrency,
        ProviderOverrides {
            provider: config.inference_provider,
            model: config.inference_model.clone(),
            endpoint: config.inference_endpoint.clone(),
            api_key_env: config.inference_api_key_env.clone(),
        },
    )
    .context("failed to initialize SIR pipeline")?;

    let mut stdout = std::io::stdout();
    for event in observer.initial_symbol_events() {
        if let Err(err) = sir_pipeline.process_event(&store, &event, config.print_sir, &mut stdout)
        {
            tracing::error!(
                file_path = %event.file_path,
                error = %err,
                "initial SIR processing error"
            );
        }
    }

    if config.lifecycle_logs {
        println!("INDEX: initial scan complete");
    }

    Ok((observer, store, sir_pipeline))
}

fn enqueue_event_paths(
    workspace: &PathBuf,
    event: notify::Result<Event>,
    queue: &mut DebounceQueue,
) -> Result<()> {
    let event = event.context("notify error")?;
    let now = Instant::now();

    for path in event.paths {
        if is_ignored_path(&path) {
            continue;
        }

        if let Ok(relative) = path.strip_prefix(workspace)
            && is_ignored_path(relative)
        {
            continue;
        }

        if path.is_dir() {
            continue;
        }

        queue.mark(path, now);
    }

    Ok(())
}
