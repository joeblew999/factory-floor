/// HTTP server — pipeline dashboard, operator upload UI, JSON API, and
/// plat-trunk job endpoints for howick-frama (Pi Zero).
///
/// Endpoints:
///   GET  /                              → redirect to /dashboard
///   GET  /dashboard                     → full pipeline status UI (auto-refreshes)
///   POST /upload                        → accept raw CSV; X-Filename header names the job
///   GET  /status                        → machine state JSON
///   GET  /jobs                          → queued + completed jobs JSON
///   GET  /health                        → health check
///
///   — plat-trunk API (called by howick-frama on Pi Zero) —
///   GET  /api/jobs/howick/pending       → next queued job for the agent
///   POST /api/jobs/howick/:id/complete  → agent marks job delivered to USB
///
///   — Phase 2: coil sensor (called by howick-frama sensor push loop) —
///   POST /api/sensor/coil               → Pi Zero pushes raw weight; server converts to metres
use std::time::SystemTime;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use opcua_howick::config::{Config, MachineConfig, SensorConfig};
use opcua_howick::machine::SharedState;

pub async fn run_http_server(
    listener: TcpListener,
    config: &Config,
    state: SharedState,
) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    tracing::info!(
        "HTTP server on http://{}/ — dashboard at http://{}/dashboard",
        addr,
        addr
    );

    let machine_config = config.machine.clone();
    let sensor_config = config.sensor.clone();

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let machine_config = machine_config.clone();
        let sensor_config = sensor_config.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, state, machine_config, sensor_config).await {
                tracing::warn!("HTTP connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    state: SharedState,
    machine_config: MachineConfig,
    sensor_config: SensorConfig,
) -> anyhow::Result<()> {
    let job_input_dir = &machine_config.job_input_dir;
    let mut buf = vec![0u8; 2 * 1024 * 1024];
    let n = stream.read(&mut buf).await?;
    buf.truncate(n);

    let raw = String::from_utf8_lossy(&buf);
    let header_end = raw.find("\r\n\r\n").unwrap_or(n);
    let headers_str = &raw[..header_end];
    let body_bytes = &buf[(header_end + 4).min(n)..];

    let first_line = headers_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    let header = |name: &str| -> Option<&str> {
        let needle = name.to_ascii_lowercase();
        headers_str.lines().skip(1).find_map(|line| {
            let colon = line.find(':')?;
            if line[..colon].trim().to_ascii_lowercase() == needle {
                Some(line[colon + 1..].trim())
            } else {
                None
            }
        })
    };

    let (status_code, content_type, body) = match (method, path) {
        // ── Dashboard (full pipeline view) ────────────────────────────────────
        ("GET", "/") => ("301", "text/plain", String::new()),

        ("GET", "/dashboard") | ("GET", "/upload") => (
            "200",
            "text/html; charset=utf-8",
            dashboard_page().to_string(),
        ),

        // ── CSV upload from browser ────────────────────────────────────────────
        // Processes jobs inline — no file-watcher dependency.
        // Watcher handles externally dropped files; dashboard uploads go direct.
        ("POST", "/upload") => {
            let filename = header("x-filename")
                .map(sanitise_filename)
                .unwrap_or_else(|| "upload.csv".into());
            let filename = if filename.ends_with(".csv") {
                filename
            } else {
                format!("{filename}.csv")
            };

            let csv = std::str::from_utf8(body_bytes)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if csv.is_empty() || !csv.starts_with("UNIT") {
                tracing::warn!("Upload rejected — not a valid Howick CSV ({})", filename);
                (
                    "400",
                    "application/json",
                    r#"{"error":"not a valid Howick CSV — file must start with UNIT,MILLIMETRE"}"#
                        .into(),
                )
            } else {
                tokio::fs::create_dir_all(job_input_dir).await?;
                let dest = job_input_dir.join(&filename);

                let frameset_name = filename.trim_end_matches(".csv").to_string();
                let job_id = format!(
                    "{}-{}",
                    frameset_name,
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );

                // Queue BEFORE writing the file: the job watcher reacts to the
                // file appearing and skips files already queued, so writing
                // first let it race in and queue the same upload a second time.
                use opcua_howick::machine::Job;
                {
                    let mut s = state.write().await;
                    s.last_upload_at = Some(SystemTime::now());
                    s.job_queue.push(Job {
                        id: job_id.clone(),
                        frameset_name: frameset_name.clone(),
                        csv_path: dest.clone(),
                        submitted_at: SystemTime::now(),
                    });
                    tracing::info!("Job {} queued (depth: {})", job_id, s.job_queue.len());
                }
                if let Err(e) = tokio::fs::write(&dest, csv.as_bytes()).await {
                    state.write().await.job_queue.retain(|j| j.id != job_id);
                    return Err(e.into());
                }
                tracing::info!("Uploaded: {} → {}", filename, dest.display());

                let body =
                    format!(r#"{{"ok":true,"frameset_name":"{frameset_name}","queued":true}}"#);
                ("200", "application/json", body)
            }
        }

        // ── Status JSON ────────────────────────────────────────────────────────
        ("GET", "/status") | ("GET", "/status?") => {
            let s = state.read().await;
            let upload_secs = ago_secs(s.last_upload_at);
            let agent_secs = ago_secs(s.agent_last_seen_at);
            let sensor_secs = ago_secs(s.sensor_last_read_at);
            let low_alert =
                s.coil_remaining_m > 0.0 && s.coil_remaining_m < sensor_config.low_alert_m;
            let body = format!(
                concat!(
                    r#"{{"version":"{version}","status":"{status}","current_job":{current_job},"#,
                    r#""pieces_produced":{pieces},"queue_depth":{queue},"#,
                    r#""coil_remaining":{coil},"coil_low_alert":{low_alert},"#,
                    r#""last_error":"{error}","#,
                    r#""last_upload_secs_ago":{upload},"completed_count":{completed},"#,
                    r#""agent_last_seen_secs_ago":{agent},"agent_last_error":"{agent_err}","#,
                    r#""sensor_last_read_secs_ago":{sensor}}}"#,
                ),
                version = opcua_howick::VERSION,
                status = s.status.as_str(),
                current_job = s
                    .current_job
                    .as_deref()
                    .map(|j| format!("\"{j}\""))
                    .unwrap_or("null".into()),
                pieces = s.pieces_produced,
                queue = s.job_queue.len(),
                coil = s.coil_remaining_m,
                low_alert = low_alert,
                error = s.last_error,
                upload = upload_secs.map(|v| v.to_string()).unwrap_or("null".into()),
                completed = s.completed_jobs.len(),
                agent = agent_secs.map(|v| v.to_string()).unwrap_or("null".into()),
                agent_err = s.agent_last_error,
                sensor = sensor_secs.map(|v| v.to_string()).unwrap_or("null".into()),
            );
            ("200", "application/json", body)
        }

        // ── Jobs JSON ──────────────────────────────────────────────────────────
        ("GET", "/jobs") => {
            let s = state.read().await;
            let completed: Vec<String> = s
                .completed_jobs
                .iter()
                .rev()
                .take(10)
                .map(|j| {
                    format!(
                        r#"{{"id":"{}","frameset_name":"{}"}}"#,
                        j.id, j.frameset_name
                    )
                })
                .collect();
            let queued: Vec<String> = s
                .job_queue
                .iter()
                .map(|j| {
                    format!(
                        r#"{{"id":"{}","frameset_name":"{}"}}"#,
                        j.id, j.frameset_name
                    )
                })
                .collect();
            let body = format!(
                r#"{{"queued":[{}],"completed":[{}]}}"#,
                queued.join(","),
                completed.join(","),
            );
            ("200", "application/json", body)
        }

        // ── Local job queue — howick-frama (Pi Zero) polls these ─────────────
        // Same API shape as plat-trunk so howick-frama works against both.
        ("GET", "/api/jobs/howick/pending") => {
            let mut s = state.write().await;
            s.agent_last_seen_at = Some(SystemTime::now());

            if let Some(job) = s.job_queue.first() {
                let id = job.id.clone();
                let frameset = job.frameset_name.clone();
                let csv_path = job.csv_path.clone();
                drop(s);
                match tokio::fs::read_to_string(&csv_path).await {
                    Ok(csv) => {
                        let escaped = csv
                            .replace('\\', "\\\\")
                            .replace('"', "\\\"")
                            .replace('\r', "")
                            .replace('\n', "\\n");
                        tracing::info!("Agent polled — serving job {id} ({frameset})");
                        let body = format!(
                            r#"{{"jobs":[{{"job_id":"{id}","frameset_name":"{frameset}","csv":"{escaped}"}}]}}"#
                        );
                        ("200", "application/json", body)
                    }
                    Err(e) => {
                        tracing::error!("Could not read CSV for job {id}: {e}");
                        (
                            "500",
                            "application/json",
                            r#"{"error":"csv read failed"}"#.into(),
                        )
                    }
                }
            } else {
                drop(s);
                tracing::debug!("Agent polled — queue empty");
                ("200", "application/json", r#"{"jobs":[]}"#.into())
            }
        }

        ("POST", p) if p.starts_with("/api/jobs/howick/") && p.ends_with("/complete") => {
            let job_id = p
                .trim_start_matches("/api/jobs/howick/")
                .trim_end_matches("/complete")
                .to_string();
            let mut s = state.write().await;
            s.agent_last_seen_at = Some(SystemTime::now());
            if let Some(pos) = s.job_queue.iter().position(|j| j.id == job_id) {
                let job = s.job_queue.remove(pos);
                tracing::info!(
                    "Agent confirmed delivery: {} ({})",
                    job_id,
                    job.frameset_name
                );
                s.completed_jobs.push(job);
                s.status = opcua_howick::machine::MachineStatus::Idle;
                s.current_job = None;
            }
            drop(s);
            ("200", "application/json", r#"{"ok":true}"#.into())
        }

        ("POST", p) if p.starts_with("/api/jobs/howick/") && p.ends_with("/error") => {
            let err = std::str::from_utf8(body_bytes)
                .unwrap_or("unknown")
                .trim()
                .to_string();
            let mut s = state.write().await;
            s.agent_last_seen_at = Some(SystemTime::now());
            s.agent_last_error = err.clone();
            drop(s);
            tracing::warn!("Agent reported error: {err}");
            ("200", "application/json", r#"{"ok":true}"#.into())
        }

        // ── Phase 2: coil sensor weight push ──────────────────────────────────
        // Pi Zero posts raw load cell weight; we convert to metres and alert.
        // Body: {"weight_kg": 23.5}
        ("POST", "/api/sensor/coil") => {
            let body_str = std::str::from_utf8(body_bytes).unwrap_or("").trim();
            // Parse weight_kg from simple JSON (no serde dependency needed)
            let weight_kg = extract_json_f64(body_str, "weight_kg");
            match weight_kg {
                Some(kg) => {
                    let metres = sensor_config.coil_metres(kg);
                    let low = metres > 0.0 && metres < sensor_config.low_alert_m;
                    {
                        let mut s = state.write().await;
                        s.coil_remaining_m = metres;
                        s.sensor_last_read_at = Some(SystemTime::now());
                    }
                    if low {
                        tracing::warn!(
                            metres = metres,
                            threshold = sensor_config.low_alert_m,
                            "⚠ Coil running low — alert fired"
                        );
                    } else {
                        tracing::info!(weight_kg = kg, metres, "Coil weight updated");
                    }
                    ("200", "application/json", r#"{"ok":true}"#.into())
                }
                None => (
                    "400",
                    "application/json",
                    r#"{"error":"expected {\"weight_kg\":23.5}"}"#.into(),
                ),
            }
        }

        ("GET", "/health") => ("200", "application/json", r#"{"ok":true}"#.into()),

        _ => ("404", "application/json", r#"{"error":"not found"}"#.into()),
    };

    let (numeric_code, reason) = match status_code {
        "200" => (200u16, "OK"),
        "301" => (301, "Moved Permanently"),
        "400" => (400, "Bad Request"),
        "500" => (500, "Internal Server Error"),
        _ => (404, "Not Found"),
    };

    let mut response = format!(
        "HTTP/1.1 {numeric_code} {reason}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {len}\r\nConnection: close\r\n",
        len = body.len(),
    );
    if numeric_code == 301 {
        response.push_str("Location: /dashboard\r\n");
    }
    response.push_str("\r\n");
    response.push_str(&body);

    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

/// Seconds since a SystemTime, or None if not set.
fn ago_secs(t: Option<SystemTime>) -> Option<u64> {
    t.and_then(|t| t.elapsed().ok()).map(|d| d.as_secs())
}

fn sanitise_filename(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

fn dashboard_page() -> &'static str {
    include_str!("../assets/dashboard.html")
}

/// Extract a JSON number value by key from a flat JSON object string.
fn extract_json_f64(json: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')?;
    rest[..end].parse::<f64>().ok()
}

#[allow(dead_code)]
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
