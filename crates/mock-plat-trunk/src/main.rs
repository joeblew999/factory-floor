//! Mock plat-trunk backend for local development.
//!
//! Simulates the two HTTP endpoints that howick-frama polls:
//!
//!   GET  /api/jobs/howick/pending          → serves real fixture CSVs in order, then empty
//!   POST /api/jobs/howick/{id}/complete    → acknowledges completion, advances queue
//!
//! Job queue (from crates/fixtures/):
//!   1. T1.csv  — roof truss,  22 components, 3945mm chords
//!   2. W1.csv  — wall frame,  42 components, 4740mm plates
//!
//! Usage (two terminals):
//!
//!   terminal 1:  mise run dev:mock    # this binary — listens on :3001 (PORT env)
//!   terminal 2:  mise run dev:agent   # howick-frama polls :3001
//!
//! Then watch ./jobs/machine/ for the written CSVs.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

struct Job {
    id: &'static str,
    frameset: &'static str,
    fixture: &'static str, // relative path from crate root
}

const JOBS: &[Job] = &[
    Job {
        id: "dev-001",
        frameset: "T1",
        fixture: "crates/fixtures/T1.csv",
    },
    Job {
        id: "dev-002",
        frameset: "W1",
        fixture: "crates/fixtures/W1.csv",
    },
];

struct Queue {
    /// Index of the next job to serve (0-based). JOBS.len() means queue exhausted.
    next: usize,
    /// True when the current job has been served but not yet acknowledged.
    pending: bool,
    /// True once we have printed the "all jobs done" message — suppresses repeat spam.
    empty_logged: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().compact().init();

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    let listener = TcpListener::bind(addr).await?;

    let queue = Arc::new(Mutex::new(Queue {
        next: 0,
        pending: false,
        empty_logged: false,
    }));

    println!("Mock plat-trunk listening on http://localhost:{port}");
    println!("  Job queue ({} jobs):", JOBS.len());
    for (i, j) in JOBS.iter().enumerate() {
        println!("    {}. {} — {}  ({})", i + 1, j.id, j.frameset, j.fixture);
    }
    println!("  Start agent: mise run dev:agent");
    println!();

    loop {
        let (stream, peer) = listener.accept().await?;
        let queue = queue.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, peer, queue).await {
                tracing::warn!("Connection error from {peer}: {e}");
            }
        });
    }
}

async fn handle(
    mut stream: tokio::net::TcpStream,
    _peer: SocketAddr,
    queue: Arc<Mutex<Queue>>,
) -> anyhow::Result<()> {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let req = std::str::from_utf8(&buf[..n]).unwrap_or("");

    let first_line = req.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    let (code, body) = match (method, path) {
        ("GET", "/api/jobs/howick/pending") => {
            let mut q = queue.lock().await;
            if q.next >= JOBS.len() || q.pending {
                if q.pending {
                    let job = &JOBS[q.next];
                    println!(
                        "  [mock] GET pending → re-serving {} (not yet acknowledged)",
                        job.id
                    );
                    // Re-serve the same job
                    match serve_job(job).await {
                        Ok(body) => (200, body),
                        Err(e) => {
                            tracing::error!("Failed to read fixture {}: {e}", job.fixture);
                            (500, r#"{"error":"fixture read failed"}"#.to_string())
                        }
                    }
                } else {
                    if !q.empty_logged {
                        println!(
                            "  [mock] GET pending → empty (all {} jobs completed — waiting for new jobs)",
                            JOBS.len()
                        );
                        q.empty_logged = true;
                    }
                    (200, r#"{"jobs":[]}"#.to_string())
                }
            } else {
                let job = &JOBS[q.next];
                println!(
                    "  [mock] GET pending → serving {} ({})",
                    job.id, job.frameset
                );
                match serve_job(job).await {
                    Ok(body) => {
                        q.pending = true;
                        (200, body)
                    }
                    Err(e) => {
                        tracing::error!("Failed to read fixture {}: {e}", job.fixture);
                        (500, r#"{"error":"fixture read failed"}"#.to_string())
                    }
                }
            }
        }

        ("POST", p) if p.contains("/complete") => {
            let mut q = queue.lock().await;
            if q.pending && q.next < JOBS.len() {
                let job = &JOBS[q.next];
                println!("  [mock] POST complete → {} acknowledged", job.id);
                q.next += 1;
                q.pending = false;
                if q.next < JOBS.len() {
                    println!("  [mock] Next job queued: {}", JOBS[q.next].id);
                } else {
                    println!("  [mock] All {} jobs done — queue empty", JOBS.len());
                }
            } else {
                println!("  [mock] POST complete → no pending job (ignored)");
            }
            (200, r#"{"ok":true}"#.to_string())
        }

        ("GET", "/health") => (200, r#"{"ok":true}"#.to_string()),

        _ => {
            tracing::debug!("404: {method} {path}");
            (404, r#"{"error":"not found"}"#.to_string())
        }
    };

    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        500 => "Internal Server Error",
        _ => "Not Found",
    };

    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len()
    );

    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

/// Read fixture file and build the JSON response body for one job.
async fn serve_job(job: &Job) -> anyhow::Result<String> {
    let csv = tokio::fs::read_to_string(job.fixture).await?;
    // Escape CSV for embedding in JSON string value
    let csv_escaped = csv
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n");
    Ok(format!(
        r#"{{"jobs":[{{"job_id":"{id}","frameset_name":"{fs}","csv":"{csv}"}}]}}"#,
        id = job.id,
        fs = job.frameset,
        csv = csv_escaped,
    ))
}
