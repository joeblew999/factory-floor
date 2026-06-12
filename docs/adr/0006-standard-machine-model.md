# ADR-0006: Standard Machine Model — Gateway + Drivers

**Status:** Proposed
**Date:** June 2026
**Context:** Splitting `opcua-howick` into a generic OPC-UA gateway + per-machine driver repos, ahead of adding robotics and SCADA to the factory.

---

## Context

Today `opcua-howick` is one workspace where "Howick" is welded into the OPC-UA
server: the node-tree is rooted at `/Howick`, and `PiecesProduced` /
`CoilRemaining` are first-class fields on `MachineState`
(`crates/core/src/machine.rs`). This works for one roll-former.

The factory is adding **many robots** and a **SCADA** layer. We do not want to
edit the server every time a machine type is added, and SCADA must not need to
know what a "Howick" is. So we need a topology where:

- one generic **gateway** owns the OPC-UA server, job queue, and dashboard;
- each **driver** (one repo per machine type) runs at the edge, translates the
  machine's native protocol, and presents a **standard node-tree** to the gateway;
- **SCADA** is just a northbound OPC-UA client of the gateway.

This ADR defines the standard machine model — the contract that makes the
gateway, dashboard, dispatcher, and SCADA code-once-run-for-all.

See ADR-0001 for the cloud/LAN/hybrid deployment topologies this sits inside.

---

## Topology

```
  MES / job source       plat-trunk ──► produces cut-lists / jobs
                                │  jobs DOWN
                                ▼
  Supervisory       SCADA ◄──── GATEWAY (one OPC-UA server) ◄──── telemetry UP
  (SCADA, HMI,     OPC-UA       /Machines/howick-1
   historian)      client       /Machines/robot-1  ...
                                  ▲           ▲
  Edge (Pi per     OPC-UA        │           │
   machine)                  howick-driver  robot-driver   ...one repo each
                                  │ native    │ native (USB / serial / ROS)
  Field                     Howick FRAMA    Robot arm
```

- **DOWN = jobs/commands.** plat-trunk → gateway job queue → driver pulls → machine.
- **UP = telemetry/status/alarms.** machine → driver → gateway nodes → SCADA + historian.

At factory scale, telemetry (UP) should move to **OPC-UA PubSub over MQTT** so a
broker fans out to SCADA + historian + dashboard. Commands (DOWN) stay
client/server (method calls / writes). Not required on day one; the model below
is written so telemetry is *published*, not polled, so the switch is non-breaking.

---

## The standard node-tree

Every machine appears under `/Machines/<MachineId>` with this layout. The
`Identity`, `Status`, and `Jobs` subtrees are **fixed** — the gateway, SCADA,
dispatcher, and dashboard are written against them and never change per machine.
The `Telemetry` subtree is **built dynamically** from the driver's declared schema.

```text
/Machines/<MachineId>/                    e.g. /Machines/howick-1
  Identity/
    MachineId   String     "howick-1"          (instance)
    Kind        String     "howick-frama"      (type)
    Vendor      String     "Howick"
    Model       String     "FRAMA 3200"
  Status/
    State          String   Offline | Idle | Running | Error
    CurrentJobId   String
    LastError      String
    AgentLastSeen  DateTime                     (edge driver heartbeat)
  Jobs/
    QueueDepth     UInt32
    CompletedCount UInt32
    Pending/
      JobId        String
      Name         String
      Payload      String/ByteString            (opaque to the gateway)
      Format       String   "text/csv" | "application/json" | "text/gcode" | ...
    CompleteJob(JobId)            Method
    FailJob(JobId, Reason)        Method
  Telemetry/                                     ← DRIVER-DECLARED
    <field> <type> (<unit>)
      Howick → PiecesProduced UInt32 · CoilRemaining Double (m)
      Robot  → CyclesCompleted UInt32 · TcpX/Y/Z Double (mm) · Temperature Double (°C)
```

### Mapping from today's `/Howick` tree

| Today (Howick-baked)        | Generic model                         | Kind   |
|-----------------------------|---------------------------------------|--------|
| `/Howick` root              | `/Machines/<id>`                      | fixed  |
| `Machine/Status`            | `Status/State`                        | fixed  |
| `Machine/CurrentJob`        | `Status/CurrentJobId`                 | fixed  |
| `Machine/LastError`         | `Status/LastError`                    | fixed  |
| `Machine/PiecesProduced`    | `Telemetry/PiecesProduced`            | driver |
| `Machine/CoilRemaining`     | `Telemetry/CoilRemaining`             | driver |
| `Jobs/QueueDepth`           | `Jobs/QueueDepth`                     | fixed  |
| `Jobs/CompletedCount`       | `Jobs/CompletedCount`                 | fixed  |
| `Jobs/PendingJobId`         | `Jobs/Pending/JobId`                  | fixed  |
| `Jobs/PendingJobName`       | `Jobs/Pending/Name`                   | fixed  |
| `Jobs/PendingJobCsv`        | `Jobs/Pending/Payload` + `Format`     | fixed  |
| `Jobs/CompleteJob(id)`      | `Jobs/CompleteJob(id)`                | fixed  |
| `agent_last_error` (HTTP)   | `Jobs/FailJob(id, reason)`            | fixed  |
| `agent_last_seen_at`        | `Status/AgentLastSeen`                | fixed  |
| `sensor_last_read_at`       | `Telemetry/*` heartbeat (driver)      | driver |

Only **two** of Howick's nodes are machine-specific, and both fall into
`Telemetry/`. Everything else is generic. Howick fits the model without special-casing.

---

## The Rust contracts

Three small types form the seam. They replace the Howick-shaped
`MachineState`/`Job` in `crates/core/src/machine.rs`.

```rust
// ── Static description: used by the GATEWAY to build the node-tree ────────────
pub struct MachineDescriptor {
    pub machine_id: String,   // "howick-1"      (instance — the /Machines/<id> key)
    pub kind:       String,   // "howick-frama"  (type)
    pub vendor:     String,   // "Howick"
    pub model:      String,   // "FRAMA 3200"
    pub job_format: JobFormat,
    pub telemetry:  Vec<TelemetryField>, // declares the Telemetry/ subtree
}

pub enum JobFormat { Csv, Json, Gcode, Opaque }

pub struct TelemetryField {
    pub name: String,           // "CoilRemaining"
    pub kind: ValueKind,        // Double
    pub unit: Option<String>,   // "m"
}

pub enum ValueKind { Bool, Int, UInt, Double, String }
pub enum Value     { Bool(bool), Int(i64), UInt(u64), Double(f64), String(String) }

// ── Generic runtime state: owned by the GATEWAY, same for every machine ───────
pub struct MachineState {
    pub status:           MachineStatus,
    pub current_job:      Option<String>,
    pub last_error:       String,
    pub job_queue:        Vec<Job>,
    pub completed_jobs:   Vec<Job>,
    pub agent_last_seen:  Option<std::time::SystemTime>,
    pub telemetry:        std::collections::BTreeMap<String, Value>, // driver fields
}

// ── Edge driver: implemented PER MACHINE, runs on the Pi ──────────────────────
#[async_trait::async_trait]
pub trait MachineDriver: Send + Sync {
    /// Static identity + telemetry schema. Drives node-tree construction.
    fn descriptor(&self) -> MachineDescriptor;

    /// Deliver one job to the physical machine. Returns when accepted.
    /// Howick: write CSV to the USB-gadget path. Robot: stream the program.
    async fn run_job(&self, job: &Job, payload: &[u8]) -> anyhow::Result<()>;

    /// Read current telemetry from the machine (sensors, counters).
    /// Returns the fields declared in `descriptor().telemetry`.
    async fn poll_telemetry(&self) -> anyhow::Result<Vec<(String, Value)>>;
}
```

What changed vs `machine.rs` today: `pieces_produced` and `coil_remaining_m`
leave the struct and become entries in `telemetry: BTreeMap<String, Value>`,
declared by `MachineDescriptor::telemetry`. Nothing else moves.

### Howick driver, sketched

```rust
impl MachineDriver for HowickFrama {
    fn descriptor(&self) -> MachineDescriptor {
        MachineDescriptor {
            machine_id: self.id.clone(),
            kind:   "howick-frama".into(),
            vendor: "Howick".into(),
            model:  self.model.clone(),
            job_format: JobFormat::Csv,
            telemetry: vec![
                TelemetryField { name: "PiecesProduced".into(), kind: ValueKind::UInt,   unit: None },
                TelemetryField { name: "CoilRemaining".into(),  kind: ValueKind::Double, unit: Some("m".into()) },
            ],
        }
    }
    async fn run_job(&self, _job: &Job, payload: &[u8]) -> anyhow::Result<()> {
        self.usb_gadget.write_csv(payload).await   // existing usb_gadget.rs path
    }
    async fn poll_telemetry(&self) -> anyhow::Result<Vec<(String, Value)>> {
        Ok(vec![
            ("PiecesProduced".into(), Value::UInt(self.counter().await?)),
            ("CoilRemaining".into(),  Value::Double(self.coil_sensor().await?)), // sensor.rs
        ])
    }
}
```

---

## Repo split this enables

Family prefix `factory-` marks the shop-floor hardware repos (house style
alongside `cf-`, `vm-`). Drivers are machine-first (`factory-<machine>-driver`).

| Repo                              | Was              | Layer                                              | Generic? |
|-----------------------------------|------------------|----------------------------------------------------|----------|
| `factory-gateway`                 | `opcua-server`   | OPC-UA server + job queue + dashboard + northbound for SCADA | yes |
| `factory-machine-model`           | generic `core`   | the three contracts above + updater/usb/http infra | yes |
| `factory-howick-driver`           | `howick-frama`   | Howick edge agent — implements `MachineDriver`     | per-machine |
| *future* `factory-<machine>-driver` | —              | next robot / CNC, same shape                        | per-machine |
| `howick-rs`                       | *(exists)*       | Frameset/CSV job payload contract                  | unchanged |
| SCADA                             | —                | northbound OPC-UA client — likely off-the-shelf    | external |

`opcua-howick` (this repo) is retired once the carve-out lands, redirecting to
the successors above. `mock-plat-trunk` folds into `factory-gateway`'s `dev/`.
Non-floor concerns stay out of the `factory-` family: `tools/speckle-*` →
its own repo later (e.g. `speckle-howick`); the plat-trunk howick **plugin**
stays in plat-trunk.

---

## Decision

Adopt the standard machine model above as the contract between gateway and
drivers. Genericize **in place** inside `opcua-howick` first (turn `machine.rs`
into the three contracts; move `/Howick` → `/Machines/<id>`; move
`PiecesProduced`/`CoilRemaining` into `Telemetry`), verify the existing Howick
pipeline still passes, **then** carve the workspace into the repos above.

## Consequences

- Adding a machine type = a new driver repo implementing `MachineDriver`. Zero
  changes to gateway, dashboard, dispatcher, or SCADA.
- The gateway's address space becomes data-driven (built from descriptors), not
  hand-coded — `build_address_space` loops over `MachineDescriptor`s.
- The HTTP routes and dashboard currently keyed on `howick` become keyed on
  `<MachineId>` (`/api/jobs/<id>/...`).
- Job payloads stay opaque to the gateway (`Payload` + `Format`); `howick-rs`
  remains the Howick-only meaning of those bytes.
```
