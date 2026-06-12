//! # howick-frama
//!
//! Minimal Howick edge agent for Raspberry Pi Zero 2W.
//!
//! The Pi Zero 2W (~$15) plugs into the Howick FRAMA machine's USB port
//! via a long cable and acts as a USB mass storage device (fake USB stick)
//! using Linux USB gadget mode (g_mass_storage kernel module).
//!
//! This binary does exactly two things:
//!   1. Subscribe to the Pi 5 OPC UA server for pending CSV jobs (push, no polling)
//!      — or fall back to HTTP polling if `plat_trunk.url` is an HTTP endpoint
//!   2. Write CSV to the USB gadget mount point + refresh USB presentation
//!
//! No HTTP server. No file watcher.
//! Binary size: ~3MB. RAM: ~16MB. Fits comfortably on Pi Zero 2W (512MB).
//!
//! For the full OPC UA server + HTTP dashboard, use opcua-server on a Pi 5,
//! NUC, or Mac Mini.
//!
//! ## Setup
//!
//! See docs/customer/06-pi-zero-setup.md for Pi Zero 2W setup guide.
//!
//! ## Config (howick-frama.dev.toml)
//!
//! ```toml
//! [machine]
//! machine_input_dir = "/mnt/usb_share"   # mounted USB image
//! usb_gadget_mode   = true               # trigger USB refresh after write
//!
//! [plat_trunk]
//! # OPC UA M2M (recommended — Pi Zero subscribes to Pi 5 OPC UA server):
//! url = "opc.tcp://howick-pi5.local:4840/"
//! # HTTP fallback (cloud plat-trunk or legacy):
//! # url = "https://your-worker.workers.dev"
//! status_push_interval_secs = 5
//! ```

use opcua_howick::{config, http_poller, machine, updater, VERSION};

mod driver;
mod edge_agent;
use edge_agent::{opcua_client, sensor};

use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("howick-frama {VERSION}");
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("howick_frama=info".parse().unwrap())
                .add_directive("opcua_howick=info".parse().unwrap()),
        )
        .compact() // compact format — saves log space on Pi Zero's SD card
        .init();

    tracing::info!(
        version = VERSION,
        "howick-frama starting (Pi Zero 2W minimal mode)"
    );

    // Background self-update check — runs once on startup.
    // On update: exit(0) so systemd restarts the new binary automatically.
    // On failure (offline, no asset, etc.): logged at debug level and ignored.
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        match updater::check_and_update(
            &client,
            "howick-frama",
            VERSION,
            "https://api.github.com",
            None,
        )
        .await
        {
            Ok(true) => {
                tracing::info!("Self-update complete — restarting");
                std::process::exit(0);
            }
            Ok(false) => {}
            Err(e) => tracing::debug!("Update check failed (offline?): {e}"),
        }
    });

    let config_path = std::env::args()
        .skip_while(|a| a != "--config")
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/howick-frama.dev.toml"));
    let config = config::Config::load_or_default(&config_path);

    // On Linux (Pi Zero) warn if USB gadget mode is not configured — it's required.
    // On other platforms (dev laptop) usb_gadget_mode = false is correct.
    #[cfg(target_os = "linux")]
    if !config.machine.usb_gadget_mode {
        tracing::warn!(
            "usb_gadget_mode = false — set true on Pi Zero 2W with machine_input_dir = /mnt/usb_share\n\
             See docs/customer/06-pi-zero-setup.md"
        );
    }

    tracing::info!(
        topology       = config.topology(),
        plat_trunk_url = %config.plat_trunk.url,
        machine_dir    = %config.machine.machine_input_dir.display(),
        usb_gadget     = config.machine.usb_gadget_mode,
        poll_interval  = config.plat_trunk.status_push_interval_secs,
        "Configuration loaded — ready to receive jobs"
    );

    // Shared machine state (lightweight — tracks job history only)
    let state = machine::new_shared_state();
    {
        let mut s = state.write().await;
        s.status = machine::MachineStatus::Idle;
    }

    // Choose transport: OPC UA subscription (M2M) or HTTP polling (cloud / legacy)
    let use_opcua = config.plat_trunk.url.starts_with("opc.tcp://");
    if use_opcua {
        tracing::info!(
            url = %config.plat_trunk.url,
            "OPC UA mode — subscribing to Pi 5 OPC UA server (no polling needed)"
        );
    } else {
        tracing::info!(
            url = %config.plat_trunk.url,
            interval = config.plat_trunk.status_push_interval_secs,
            "HTTP mode — polling plat-trunk every {}s",
            config.plat_trunk.status_push_interval_secs,
        );
    }

    // Phase 2: coil sensor push loop (only when sensor.enabled = true in config)
    if config.sensor.enabled {
        let sensor_url = config.plat_trunk.url.clone();
        let sensor_interval = config.sensor.poll_interval_secs;
        tracing::info!(
            poll_interval = sensor_interval,
            "Coil sensor enabled — pushing weight to {sensor_url}"
        );
        tokio::select! {
            r = run_job_transport(config, state, use_opcua) => {
                if let Err(e) = r { tracing::error!("Job transport failed: {e}"); }
            }
            r = sensor::run_sensor_push(sensor_url, sensor_interval) => {
                if let Err(e) = r { tracing::error!("Sensor push failed: {e}"); }
            }
        }
    } else if let Err(e) = run_job_transport(config, state, use_opcua).await {
        tracing::error!("Job transport failed: {e}");
        std::process::exit(1);
    }

    Ok(())
}

/// Run the appropriate job transport based on the server URL scheme.
/// - `opc.tcp://` → OPC UA subscription (event-driven, no polling)
/// - `http://` / `https://` → HTTP polling (cloud plat-trunk or legacy)
async fn run_job_transport(
    config: config::Config,
    state: machine::SharedState,
    use_opcua: bool,
) -> anyhow::Result<()> {
    if use_opcua {
        opcua_client::run_opcua_agent(config, state).await
    } else {
        http_poller::run_job_poller(config, state).await
    }
}
