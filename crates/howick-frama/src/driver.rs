//! Howick FRAMA driver — the first [`MachineDriver`] implementation (ADR-0006).
//!
//! This is the proof that the Howick machine fits the generic machine model:
//! everything Howick-specific lives here, behind the `MachineDriver` trait, so
//! the gateway never names "Howick" in its own code. When this driver moves to
//! its own `howick-driver` repo, this file is the whole of what moves.
//!
//! - `descriptor()`  → identity + the two Howick-specific telemetry fields.
//! - `run_job()`     → writes the cut-list CSV to the USB-gadget mount.
//! - `poll_telemetry()` → reads the coil load-cell, converts kg → metres.
//!
//! Scaffold: defines the seam for the repo split. Not yet wired into the agent
//! run-loop (which still uses the direct `opcua_client` path), hence `dead_code`.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use opcua_howick::config::{MachineConfig, SensorConfig};
use opcua_howick::machine_model::{
    JobFormat, JobPayload, MachineDescriptor, MachineDriver, Telemetry, TelemetryField, Value,
    ValueKind,
};
use opcua_howick::usb_gadget;

use crate::edge_agent::sensor;

/// Telemetry field names — must match the descriptor and the gateway nodes.
pub const PIECES_PRODUCED: &str = "PiecesProduced";
pub const COIL_REMAINING: &str = "CoilRemaining";

/// Driver for a single Howick FRAMA roll-former wired to this Pi.
pub struct HowickFrama {
    machine_id: String,
    model: String,
    machine_config: MachineConfig,
    sensor_config: SensorConfig,
    pieces_produced: AtomicU64,
}

impl HowickFrama {
    pub fn new(
        machine_id: impl Into<String>,
        machine: MachineConfig,
        sensor: SensorConfig,
    ) -> Self {
        Self {
            machine_id: machine_id.into(),
            model: "FRAMA".to_owned(),
            machine_config: machine,
            sensor_config: sensor,
            pieces_produced: AtomicU64::new(0),
        }
    }
}

impl MachineDriver for HowickFrama {
    fn descriptor(&self) -> MachineDescriptor {
        MachineDescriptor {
            machine_id: self.machine_id.clone(),
            kind: "howick-frama".to_owned(),
            vendor: "Howick".to_owned(),
            model: self.model.clone(),
            job_format: JobFormat::Csv,
            telemetry: vec![
                TelemetryField::new(PIECES_PRODUCED, ValueKind::UInt, None),
                TelemetryField::new(COIL_REMAINING, ValueKind::Double, Some("m")),
            ],
        }
    }

    async fn run_job(&self, job: &JobPayload) -> anyhow::Result<()> {
        let csv = std::str::from_utf8(&job.bytes)
            .map_err(|e| anyhow::anyhow!("job {} payload is not UTF-8 CSV: {e}", job.job_id))?;
        let filename = format!("{}.csv", job.name);
        usb_gadget::write_job(&self.machine_config, &filename, csv).await?;
        // Each delivered job is one produced piece-set on this machine.
        self.pieces_produced.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn poll_telemetry(&self) -> anyhow::Result<Telemetry> {
        let mut t = Telemetry::new();
        t.insert(
            PIECES_PRODUCED.to_owned(),
            Value::UInt(self.pieces_produced.load(Ordering::Relaxed)),
        );
        if self.sensor_config.enabled {
            if let Some(kg) = sensor::read_weight_kg() {
                let metres = self.sensor_config.coil_metres(kg);
                t.insert(COIL_REMAINING.to_owned(), Value::Double(metres));
            }
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> HowickFrama {
        HowickFrama::new(
            "howick-1",
            MachineConfig {
                machine_name: "Howick FRAMA".into(),
                job_input_dir: "/tmp/in".into(),
                machine_input_dir: "/tmp/machine".into(),
                machine_output_dir: "/tmp/out".into(),
                usb_gadget_mode: false,
            },
            SensorConfig::default(),
        )
    }

    #[test]
    fn descriptor_declares_howick_telemetry() {
        let d = driver().descriptor();
        assert_eq!(d.kind, "howick-frama");
        assert_eq!(d.job_format, JobFormat::Csv);
        let names: Vec<&str> = d.telemetry.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, [PIECES_PRODUCED, COIL_REMAINING]);
    }

    #[tokio::test]
    async fn telemetry_reports_pieces_even_with_sensor_off() {
        let d = driver(); // SensorConfig::default() → disabled
        let t = d.poll_telemetry().await.unwrap();
        assert_eq!(t.get(PIECES_PRODUCED), Some(&Value::UInt(0)));
        assert!(
            !t.contains_key(COIL_REMAINING),
            "no coil node when sensor off"
        );
    }
}
