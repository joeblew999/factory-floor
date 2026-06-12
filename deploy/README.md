# Run / deploy with Docker

The whole factory stack — gateway + machine agent — in **one command**. No Rust,
no other repos, no mise. The images build straight from GitHub.

## Run it

```bash
docker compose up --build      # first time builds the images, then starts everything
```

Then open the dashboard: **http://localhost:4841/**

Send a test cut-list to the machine:

```bash
curl -X POST http://localhost:4841/jobs/howick-1 --data-binary $'UNIT,MILLIMETRE\nW1,4740\n'
```

It travels gateway → edge-agent → the machine. See it land:

```bash
docker compose exec edge-agent ls -la /cutlists
```

Stop:

```bash
docker compose down
```

## Deploy to a real factory server

It's the **same file**. On the server:

```bash
git clone https://github.com/joeblew999/factory-floor.git
cd factory-floor/deploy
docker compose up --build -d   # -d = run in the background
```

Edit [`gateway.toml`](gateway.toml) (which machines this factory has) and
[`agent.toml`](agent.toml) (where the machine's USB mount is) for the real site.

## What's here

| File | What |
|------|------|
| [`docker-compose.yml`](docker-compose.yml) | the two services + how they connect |
| [`gateway.toml`](gateway.toml) | this factory's machines |
| [`agent.toml`](agent.toml) | the machine agent's settings |

> Faster later: once CI publishes pre-built images to ghcr, `--build` won't be
> needed — `docker compose up` just pulls and runs.
