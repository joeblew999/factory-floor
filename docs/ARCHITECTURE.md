# Architecture

The technical detail behind [the README](../README.md). Read that first for the
plain version.

## The family of repos

| Repo | Role |
|------|------|
| **factory-floor** | umbrella — this README + docs + the local run harness (`run/`) |
| [factory-machine-model](https://github.com/joeblew999/factory-machine-model) | the contract: OPC-UA Machinery + ISA-95 modelled in Rust, + the `MachineDriver` trait |
| [factory-gateway](https://github.com/joeblew999/factory-gateway) | the OPC-UA server — one per factory; driver registry + ISA-95 job control + dashboard |
| [factory-howick-driver](https://github.com/joeblew999/factory-howick-driver) | the Howick FRAMA machine driver (first of many) |
| [factory-edge-agent](https://github.com/joeblew999/factory-edge-agent) | runs a driver at the machine, connected to the gateway over OPC-UA |
| `factory-<machine>-driver` | one per machine type — composed per factory |

## How it fits together

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

**Deployment** — two modes per machine (set `edge` in config):
- `edge = true` (real factory floor): the driver runs in a
  [factory-edge-agent](https://github.com/joeblew999/factory-edge-agent) process
  on the box wired to the machine; the gateway publishes jobs to it over OPC-UA and
  it reports back. Machines can be physically distributed; a crashed agent can't
  take down the rest.
- `edge = false` (single co-located machine): the driver runs in-process in the
  gateway. Simpler, no separate process.

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
              JobState: NotAllowedToStart → AllowedToStart → Running → Completed | Aborted | Interrupted
```

- **OPC UA for Machinery (OPC 40001-1)** → machine identity + state.
  <https://reference.opcfoundation.org/Machinery/v103/docs/>
- **OPC UA for ISA-95 Job Control (OPC 10031-4)** → job dispatch.
  <https://reference.opcfoundation.org/ISA95JOBCONTROL/docs/>

The machine payload (a Howick cut-list CSV, a robot program, …) rides inside an
ISA-95 `JobOrder` as a parameter — opaque to the gateway, meaningful only to the driver.

## Local development across repos

Each repo builds standalone (drivers + gateway depend on `factory-machine-model`
via git). To iterate across repos without GitHub round-trips, clone them as siblings
and add a dev-only patch so cargo uses your local checkouts:

```toml
# factory-gateway/.cargo/config.toml  (dev only — don't commit)
[patch."https://github.com/joeblew999/factory-machine-model"]
factory-machine-model = { path = "../factory-machine-model" }
[patch."https://github.com/joeblew999/factory-howick-driver"]
factory-howick-driver = { path = "../factory-howick-driver" }
```

## Design records

- [ADR-0006](adr/0006-standard-machine-model.md) — the `MachineDriver` seam (+ the workspace-vs-repos history).
- [ADR-0007](adr/0007-opcua-companion-specs-and-fleet.md) — OPC-UA companion specs; the multi-factory fleet; standards-driven config.

## What's reused vs built (don't reinvent)

The spine we build is small: the **drivers**, the gateway's **job orchestration**,
the **config model**, and the **CAD→cut-list bridge**. Everything else is standards
or off-the-shelf: OPC-UA stack (`async-opcua`), SCADA (FUXA/Ignition), historian
(InfluxDB), messaging (MQTT+Sparkplug), identity (Rauthy), PKI (OPC-UA GDS). If we
find ourselves writing any of those, stop.
