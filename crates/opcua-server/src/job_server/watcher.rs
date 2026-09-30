use std::path::Path;
use std::time::SystemTime;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use opcua_howick::config::MachineConfig;
use opcua_howick::machine::{Job, MachineStatus, SharedState};

/// Watch the job input directory for new CSV files.
/// When a CSV arrives, copy it to the machine input directory and update state.
pub async fn run_job_watcher(config: MachineConfig, state: SharedState) -> anyhow::Result<()> {
    // Ensure directories exist
    tokio::fs::create_dir_all(&config.job_input_dir).await?;
    tokio::fs::create_dir_all(&config.machine_input_dir).await?;
    tokio::fs::create_dir_all(&config.machine_output_dir).await?;

    tracing::info!("Watching for jobs in: {}", config.job_input_dir.display());
    tracing::info!(
        "Machine input dir:    {}",
        config.machine_input_dir.display()
    );

    let (tx, mut rx) = mpsc::channel(32);

    // Set up file system watcher
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<Event>| {
            if let Ok(event) = result {
                let _ = tx.blocking_send(event);
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(&config.job_input_dir, RecursiveMode::NonRecursive)?;

    // Process file system events
    while let Some(event) = rx.recv().await {
        if let EventKind::Create(_) = event.kind {
            for path in event.paths {
                if is_csv(&path) {
                    tracing::info!("New job file detected: {}", path.display());
                    if let Err(e) = handle_new_job(&path, &state).await {
                        tracing::error!("Failed to process job {}: {e}", path.display());
                    }
                }
            }
        }
    }

    Ok(())
}

fn is_csv(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("csv"))
        .unwrap_or(false)
}

async fn handle_new_job(csv_path: &Path, state: &SharedState) -> anyhow::Result<()> {
    // Derive job ID and frameset name from filename (e.g. "W1.csv" → "W1")
    let frameset_name = csv_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Skip if this file is already queued (e.g. dashboard upload already added it).
    // Compare file names, not full paths: the watcher reports the resolved path
    // (macOS: /private/var/...) while the upload handler used the symlinked one
    // (/var/...), so a full-path compare never matched and queued it twice.
    {
        let s = state.read().await;
        if s.job_queue
            .iter()
            .any(|j| j.csv_path.file_name() == csv_path.file_name())
        {
            tracing::debug!(
                "Skipping {} — already queued by upload handler",
                csv_path.display()
            );
            return Ok(());
        }
    }

    let job_id = format!("{}-{}", frameset_name, timestamp_id());

    // Hold in queue — howick-frama (Pi Zero) picks up via HTTP and writes to USB
    let job = Job {
        id: job_id.clone(),
        frameset_name: frameset_name.clone(),
        csv_path: csv_path.to_path_buf(),
        submitted_at: SystemTime::now(),
    };
    let mut s = state.write().await;
    s.job_queue.push(job);
    s.status = MachineStatus::Idle;
    tracing::info!(
        "Job {} queued for agent pickup (depth: {})",
        job_id,
        s.job_queue.len()
    );

    Ok(())
}

fn timestamp_id() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}
