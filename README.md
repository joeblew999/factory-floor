# factory-floor

<https://github.com/joeblew999/factory-floor>

Open factory-floor automation over **OPC-UA**. A fleet of factories, each with a
different mix of hardware (roll-formers, robots, CNCs), every machine exposed on a
**standard OPC-UA address space** so any conformant SCADA / MES / historian
interoperates with no bespoke integration.

This repo is the **umbrella**: the architecture, the ADRs, and the local
dev workspace. The code lives in the `factory-` family of repos.

## The family

| Repo | Role |
|------|------|
| **factory-floor** (this) | umbrella — architecture, ADRs, local dev workspace |
| [factory-machine-model](https://github.com/joeblew999/factory-machine-model) | the contract: OPC-UA Machinery + ISA-95 modelled in Rust, + the `MachineDriver` trait |
| [factory-gateway](https://github.com/joeblew999/factory-gateway) | the OPC-UA server — one per factory; driver registry + ISA-95 job control |
| [factory-howick-driver](https://github.com/joeblew999/factory-howick-driver) | the Howick FRAMA machine driver (first of many) |
| `factory-<machine>-driver` | one per machine type — composed per factory |
| [howick-rs](https://github.com/joeblew999/howick-rs) | the Howick cut-list / CSV payload library |

## Architecture

```
   MES / cloud ── ISA-95 JobOrders ──┐
                                      ▼
  Factory A gateway          Factory B gateway              one OPC-UA server per factory
   Machines/ + JobControl      Machines/ + JobControl        (OPC UA for Machinery + ISA-95)
     ├ howick-1 → howick driver   ├ press-1 → press driver
     └ robot-1  → kuka driver     └ cnc-1   → machinetool driver
   SCADA (OPC-UA client) subscribes per factory; historian logs telemetry.
```

The **driver is the unit of variation**: each is an independently-versioned plugin
crate, and **each factory's config selects which drivers + machines it runs.** Add
a machine type → a new driver crate. Add a factory → a new config file. The gateway
never changes.

## The OPC-UA structure (the standards we build on)

We don't invent the node-tree — we use two official OPC-UA **companion
specifications**, so off-the-shelf industrial tooling already understands it:

```text
Objects/
└── Machines/                              ← OPC UA for Machinery (OPC 40001-1)
    └── <machine-id>/
        ├── Identification/   Manufacturer · Model · SerialNumber · DeviceClass · …   (standard nameplate)
        ├── MachineryItemState   NotAvailable | OutOfService | NotExecuting | Executing
        ├── Telemetry/         driver-declared, e.g. Howick → PiecesProduced · CoilRemaining
        └── JobOrderReceiver   ← OPC UA for ISA-95 Job Control (OPC 10031-4)
              Store · StoreAndStart · Start · Stop · Cancel · Pause · Resume · Abort · Clear
              JobOrder{ JobOrderID, Description, WorkMasterID, parameters[] }
              JobState: Stored → Queued → Running → Ended | Aborted | Interrupted
```

- **OPC UA for Machinery (OPC 40001-1)** → machine identity + state.
  <https://reference.opcfoundation.org/Machinery/v103/docs/>
- **OPC UA for ISA-95 Job Control (OPC 10031-4)** → job dispatch.
  <https://reference.opcfoundation.org/ISA95JOBCONTROL/docs/>

The machine payload (a Howick cut-list CSV, a robot program, …) rides inside an
ISA-95 `JobOrder` as a parameter — opaque to the gateway, meaningful only to the driver.

## Local development

Each repo builds standalone (drivers + gateway depend on `factory-machine-model`
via git). To iterate across repos without GitHub round-trips, clone them as
siblings and add a dev-only patch so cargo uses your local checkouts:

```toml
# factory-gateway/.cargo/config.toml  (dev only — don't commit)
[patch."https://github.com/joeblew999/factory-machine-model"]
factory-machine-model = { path = "../factory-machine-model" }
[patch."https://github.com/joeblew999/factory-howick-driver"]
factory-howick-driver = { path = "../factory-howick-driver" }
```

GitHub only matters at release; day-to-day you build against local paths.

## Design records

- [ADR-0006](docs/adr/0006-standard-machine-model.md) — the `MachineDriver` seam (+ the workspace-vs-repos history).
- [ADR-0007](docs/adr/0007-opcua-companion-specs-and-fleet.md) — grounding in OPC-UA companion specs; the multi-factory fleet; standards-driven config.

## Legacy

`crates/` here is the original single-binary Howick implementation (`opcua-server`
+ `howick-frama`) that the family was extracted from. It remains the reference for
the parts not yet ported to the family (the web dashboard, the edge OPC-UA client,
the plat-trunk poller). Being superseded by the repos above.
