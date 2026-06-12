# ADR-0007: Ground the model in OPC-UA companion specs; multi-factory fleet; config from the standard

**Status:** Accepted
**Date:** June 2026
**Supersedes:** the invented node-tree in ADR-0006 (the *seam* from 0006 — the
`MachineDriver` trait — stands; its field/state/method **names** are replaced by
the standard ones below).

---

## Context

This is not a single machine — it is a **fleet of factories**, each with a
*different* mix of hardware (Howick roll-formers, robots, CNCs, presses) and
therefore a *different* set of drivers. ADR-0006 invented an Identity/Status/Jobs
node-tree. That was avoidable: OPC-UA already standardises exactly this problem in
two companion specifications. Building on the standards means our gateway speaks
the same language as any off-the-shelf SCADA, MES, or historian the customer buys.

## The governing standards

- **OPC UA for Machinery — OPC 40001-1 / VDMA 40001-1.** A factory of many machines:
  a well-known **`Machines`** folder listing every machine instance; per-machine
  **`Identification`** nameplate (Manufacturer, ManufacturerUri, Model, ProductCode,
  ProductInstanceUri, SerialNumber, DeviceClass, HardwareRevision, SoftwareRevision,
  YearOfConstruction, Location…); a **`Components`** tree; and a standard
  **`MachineryItemState`** state machine (`NotAvailable` / `OutOfService` /
  `NotExecuting` / `Executing`, plus an operation-mode sub-state).
  <https://reference.opcfoundation.org/Machinery/v103/docs/>
- **OPC UA for ISA-95 Job Control — OPC 10031-4 (v2.0, 2024).** Dispatching work to
  machines: a **JobOrderReceiver** object with methods **Store, StoreAndStart,
  Start, Stop, Cancel, Pause, Resume, Abort, Clear**; an **ISA95JobOrder**
  (JobOrderID, Description, WorkMasterID, JobOrderParameters…); and queued /
  running / completed job states with JobResponses.
  <https://reference.opcfoundation.org/ISA95JOBCONTROL/docs/>

**Proper = Machinery for identity + state, ISA-95 for jobs.** Per-domain specs
layer on top per driver (Robotics OPC 40010, Machine Tools / umati OPC 40501, …).

## Decision

### 1. Model on the standards (replaces ADR-0006's invented tree)

| ADR-0006 (invented)         | OPC 40001-1 / 10031-4 (standard)                        |
|-----------------------------|--------------------------------------------------------|
| `/Machines/<id>` *(kept — it was right)* | `Machines/` folder (Machinery)            |
| `Identity/*`                | `Identification` nameplate (Machinery)                 |
| `Status/State`              | `MachineryItemState` state machine (Machinery)         |
| `Telemetry/*`               | machine-specific vars + per-domain companion spec      |
| `Jobs/Pending` + `CompleteJob` | `JobOrderReceiver`: Store / Start / … (ISA-95)      |
| `JobPayload`                | `ISA95JobOrder` (payload carried as a JobOrderParameter)|

The `MachineDriver` trait from ADR-0006 stays as the Rust seam; `descriptor()`
now returns standard `Identification` fields + declared `MachineryItemState`
capability, and job hand-off is modelled as an ISA-95 JobOrder.

### 2. Topology — one gateway per factory, drivers composed per factory

```
   MES / cloud (plat-trunk) ── ISA-95 JobOrders ──┐
                                                   ▼
  Factory A gateway                Factory B gateway          (one OPC-UA server each:
   Machines/ + JobOrderReceiver     Machines/ + JobOrderReceiver  Machinery + Job Control)
     ├ howick-1 → howick driver       ├ press-1 → press driver
     └ robot-1  → kuka driver         └ cnc-1   → machinetool driver
   SCADA (OPC-UA client) subscribes per factory; historian logs telemetry.
```

The **driver is the unit of variation.** Drivers are independently-versioned
**plugin crates**; each factory's **config** selects which drivers + machine
instances it runs. This is fleet composability without a repo per driver.

### 3. One cargo workspace (`factory-floor`)

Develop in a single workspace; drivers are crates, not repos (ADR-0006 amendment).
Independent versioning is per-crate, not per-repo. A driver graduates to its own
repo only when a vendor/team owns it — the stable `factory-machine-model` contract
makes that a drop-in (then dev-only cargo `[patch]` over local checkouts).

### 4. Config is the standard, instantiated per factory

The config **is** that factory's Machinery `Machines` folder. Generic shape from
the standard nameplate; each driver contributes a typed sub-section:

```toml
[factory]
id   = "si-racha"
name = "Prin — Si Racha"

[[machine]]                      # → one entry under Machines/ in the address space
id     = "howick-1"
driver = "howick-frama"          # which plugin crate handles it
[machine.identification]         # → OPC UA Machinery Identification nameplate
manufacturer  = "Howick"
model         = "FRAMA"
serial_number = "FR-2231"
[machine.howick]                 # driver-specific, typed by the howick driver
usb_mount   = "/mnt/usb_share"
coil_sensor = true

[[machine]]
id     = "robot-1"
driver = "kuka"
[machine.identification]
manufacturer = "KUKA"
model        = "KR 10"
[machine.kuka]
endpoint = "opc.tcp://kuka-1.local:4840"
```

Generic config = `[factory]` + `[[machine]]` list + standard `[machine.identification]`.
Driver-specific config = `[machine.<driver>]`, a schema each driver declares (parsed
by that driver, opaque to the gateway). The earlier flat `Config`
(`opcua`/`machine`/`sensor`/`plat_trunk`/`http`) collapses into: gateway-level
settings + this per-machine list; `SensorConfig` moves into `[machine.howick]`.

### 5. Naming (the family)

| Name | Role |
|------|------|
| `factory-floor` | the one workspace repo (was `opcua-howick`) |
| `factory-machine-model` | the Machinery + ISA-95 contract crate |
| `factory-gateway` | OPC-UA server — `Machines/` + `JobOrderReceiver` |
| `factory-howick-driver` (`crates/howick-driver`) | Howick driver plugin |
| `factory-<hw>-driver` | future per-hardware drivers |
| `howick-rs` | Howick cut-list / CSV payload lib (unchanged) |

## Consequences

- The gateway exposes a **standard** address space — any conformant SCADA / MES /
  historian interoperates without bespoke integration.
- Adding a machine type = a new driver crate + `[[machine]]` config entries. No
  gateway change.
- Adding a *factory* = a new config file (its machine/driver set). No code change.
- We must implement against the Machinery + ISA-95 nodesets (companion-spec
  nodeset XML), not hand-rolled nodes — more upfront work, standards-correct output.
- `howick-rs` cut-list bytes become a JobOrderParameter inside an ISA-95 JobOrder.
