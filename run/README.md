# run/ — local stack (mise + pitchfork)

Runs the whole factory-floor stack on your machine: the **gateway** plus one
**edge agent** driving a Howick machine, exactly as they'd run on a real floor
(two processes, talking over OPC-UA).

Binaries are built from the sibling family repos checked out next to this one
(`../../factory-gateway`, `../../factory-edge-agent`).

```bash
cd run
mise run up      # build the binaries + start gateway & edge-agent as pitchfork daemons
mise run demo    # POST a cut-list → watch it flow HTTP → ISA-95 → OPC-UA → the machine
mise run status  # daemon + live machine status
mise run logs    # daemon logs
mise run down    # stop everything
```

`mise run demo` POSTs a cut-list to the gateway's HTTP endpoint; that becomes an
ISA-95 `StoreAndStart` on the machine's JobOrderReceiver; the gateway publishes it
over OPC-UA; the edge agent picks it up, runs the driver, and writes the cut-list
to `/tmp/factory-usb/` (the stand-in for the FRAMA's USB mount); then reports
complete, and the gateway closes the job. Dashboard: <http://127.0.0.1:4841/>.

Files: [`gateway.toml`](gateway.toml) (the factory + its machines),
[`agent.toml`](agent.toml) (one edge agent), [`pitchfork.toml`](pitchfork.toml)
(the daemons), [`mise.toml`](mise.toml) (the tasks).

> Note: run tasks by name from this directory. The repo root still contains the
> **legacy** `opcua-howick` mise/pitchfork config (being retired); `mise run up`
> here starts the `gateway`/`edge-agent` daemons explicitly to avoid it.
