# factory-floor

**Send a cut-list to a factory machine automatically — no USB-stick walk.**

You drop in a job (a cut-list file). It travels across the factory network and the
machine starts making it. That's the whole thing.

```
   you drop a cut-list  ───►  gateway  ───►  the machine makes it
```

It works today. To see the whole thing run on your own computer — **one command**:

```bash
cd run
mise run try     # builds + starts everything + sends a test cut-list + shows it arrive
```

Then when you're done:

```bash
mise run down    # stops it
```

(Or step by step: `mise run up`, `mise run demo`, `mise run down`.)

---

## The pieces (you rarely need to think about these)

- **factory-gateway** — the brain. Runs once per factory.
- **factory-edge-agent** — a small worker that sits at each machine.
- **factory-howick-driver** — knows how to talk to a Howick machine. (More machine types = more drivers.)
- **factory-machine-model** — the shared language the three speak.

The other repos are support: this one (docs + how to run it),
[factory-customers](https://github.com/joeblew999/factory-customers) (private customer files),
and [speckle](https://github.com/joeblew999/speckle) (the design → cut-list tooling).

Adding a new factory = a new config file. Adding a new machine type = a new driver.
The gateway never changes.

---

## Want the technical details?

It's built on **OPC-UA**, the standard industrial protocol — so off-the-shelf
factory software (SCADA, dashboards, etc.) can plug in later without custom work.

- [ARCHITECTURE.md](docs/ARCHITECTURE.md) — how it fits together, the OPC-UA model, the standards.
- [ADR-0006](docs/adr/0006-standard-machine-model.md) / [ADR-0007](docs/adr/0007-opcua-companion-specs-and-fleet.md) — the design decisions and why.

## Legacy

`crates/` is the original single-binary version this was built from. Being retired.
