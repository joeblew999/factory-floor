//! Standard machine model — the gateway⇄driver contract (ADR-0006).
//!
//! This is the seam that lets one generic OPC-UA gateway serve many machine
//! types (Howick roll-formers, robots, CNCs) and lets SCADA speak to all of
//! them without knowing what any of them is.
//!
//! Three pieces:
//!   - [`MachineDescriptor`] — static identity + telemetry schema. The gateway
//!     uses it to **build** the `/Machines/<id>` node-tree. Driver-supplied.
//!   - [`MachineDriver`] — runtime behaviour, implemented **per machine type**
//!     and run at the edge (e.g. on the Pi wired to the machine).
//!   - [`Value`] / [`TelemetryField`] — the dynamic `Telemetry/` subtree: the
//!     only part of the node-tree that varies between machine types.
//!
//! Everything else (Status, Jobs, CompleteJob/FailJob) is identical for every
//! machine and lives in [`crate::machine::MachineState`].

use std::collections::BTreeMap;

/// Static description of a machine type. Stable for the machine's lifetime.
///
/// The gateway reads this to construct the OPC-UA address space and the
/// dashboard; SCADA reads `Identity/*` to label devices. A driver returns one
/// from [`MachineDriver::descriptor`].
#[derive(Debug, Clone, PartialEq)]
pub struct MachineDescriptor {
    /// Instance id — the `/Machines/<machine_id>` key. e.g. `"howick-1"`.
    pub machine_id: String,
    /// Machine type. e.g. `"howick-frama"`, `"robot-arm"`.
    pub kind: String,
    /// Manufacturer. e.g. `"Howick"`.
    pub vendor: String,
    /// Model. e.g. `"FRAMA 3200"`.
    pub model: String,
    /// How the opaque job payload (`Jobs/Pending/Payload`) is encoded.
    pub job_format: JobFormat,
    /// Declares the `Telemetry/` subtree for this machine type.
    pub telemetry: Vec<TelemetryField>,
}

/// Encoding of a job payload. Opaque to the gateway — only the driver and the
/// upstream producer (e.g. `howick-rs`) understand the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFormat {
    /// Howick cut-list CSV (`text/csv`).
    Csv,
    /// JSON job document (`application/json`).
    Json,
    /// G-code program (`text/gcode`).
    Gcode,
    /// Anything else — bytes with no gateway-side meaning.
    Opaque,
}

impl JobFormat {
    /// MIME-ish tag published at `Jobs/Pending/Format`.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobFormat::Csv => "text/csv",
            JobFormat::Json => "application/json",
            JobFormat::Gcode => "text/gcode",
            JobFormat::Opaque => "application/octet-stream",
        }
    }
}

/// One driver-declared telemetry node (becomes `Telemetry/<name>`).
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryField {
    /// OPC-UA BrowseName, e.g. `"CoilRemaining"`.
    pub name: String,
    /// Value type — drives the OPC-UA DataType of the node.
    pub kind: ValueKind,
    /// Engineering unit for display, e.g. `Some("m")`. `None` for counts.
    pub unit: Option<String>,
}

impl TelemetryField {
    pub fn new(name: impl Into<String>, kind: ValueKind, unit: Option<&str>) -> Self {
        Self {
            name: name.into(),
            kind,
            unit: unit.map(str::to_owned),
        }
    }
}

/// Type tag for a telemetry field / [`Value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    Int,
    UInt,
    Double,
    String,
}

/// A telemetry reading. The dynamic half of machine state.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Double(f64),
    String(String),
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Bool(_) => ValueKind::Bool,
            Value::Int(_) => ValueKind::Int,
            Value::UInt(_) => ValueKind::UInt,
            Value::Double(_) => ValueKind::Double,
            Value::String(_) => ValueKind::String,
        }
    }

    /// Numeric coercion to f64 for dashboard / JSON output (`None` for strings/bools).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::UInt(v) => Some(*v as f64),
            Value::Double(v) => Some(*v),
            _ => None,
        }
    }
}

/// Telemetry snapshot — `field name → value`. Stored on the gateway's machine
/// state and synced to the `Telemetry/` nodes each tick.
pub type Telemetry = BTreeMap<String, Value>;

/// A job handed to a driver. The payload bytes are opaque (see [`JobFormat`]).
#[derive(Debug, Clone, PartialEq)]
pub struct JobPayload {
    pub job_id: String,
    pub name: String,
    pub format: JobFormat,
    pub bytes: Vec<u8>,
}

/// Per-machine-type runtime behaviour. Implemented once per machine and run at
/// the edge. The gateway never depends on a concrete impl — only this trait —
/// which is what allows each driver to live in its own repo.
///
/// Object-safe via `&dyn` is **not** required; the gateway selects a driver at
/// startup and uses it through generics / a boxed future as needed.
pub trait MachineDriver: Send + Sync {
    /// Static identity + telemetry schema. Drives node-tree construction.
    fn descriptor(&self) -> MachineDescriptor;

    /// Deliver one job to the physical machine. Resolves when the machine has
    /// accepted it (e.g. Howick: CSV written to the USB-gadget path).
    fn run_job(
        &self,
        job: &JobPayload,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Read current telemetry from the machine. The returned field names must
    /// match those declared in [`MachineDescriptor::telemetry`].
    fn poll_telemetry(&self)
        -> impl std::future::Future<Output = anyhow::Result<Telemetry>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_kind_roundtrips() {
        assert_eq!(Value::UInt(3).kind(), ValueKind::UInt);
        assert_eq!(Value::Double(1.5).as_f64(), Some(1.5));
        assert_eq!(Value::String("x".into()).as_f64(), None);
    }

    #[test]
    fn job_format_tags() {
        assert_eq!(JobFormat::Csv.as_str(), "text/csv");
    }
}
