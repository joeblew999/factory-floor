/// Integration tests for the full Option B pipeline.
///
/// These tests exercise the same HTTP endpoints that:
///   - the dashboard calls (GET /status, GET /jobs, POST /upload)
///   - howick-frama calls (GET /api/jobs/howick/pending, POST /api/jobs/howick/:id/complete)
///   - the coil sensor calls (POST /api/sensor/coil)
///
/// Each test starts a real HTTP server + file watcher on a temp directory,
/// makes real HTTP requests, and checks the results.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::time::sleep;

use opcua_howick::config::{
    Config, HttpConfig, MachineConfig, OpcUaConfig, PlatTrunkConfig, SensorConfig,
};
use opcua_howick::machine::{new_shared_state, MachineStatus};
use opcua_server::job_server::http::run_http_server;
use opcua_server::job_server::watcher::run_job_watcher;

// ── Fixture CSV ────────────────────────────────────────────────────────────────

const T1_CSV: &str = include_str!("../../fixtures/T1.csv");

// ── Test helpers ───────────────────────────────────────────────────────────────

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Returns fresh, empty temp input and machine directories for this test.
/// Keyed by pid as well as counter, and wiped first, so files left by an
/// earlier run can never leak into this one's job queue.
fn test_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("howick-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    (base.join("input"), base.join("machine"))
}

fn make_config(job_input_dir: std::path::PathBuf, machine_input_dir: std::path::PathBuf) -> Config {
    Config {
        opcua: OpcUaConfig {
            host: "0.0.0.0".into(),
            port: 4840,
            application_name: "test".into(),
            namespace_uri: "urn:howick-frama".into(),
        },
        machine: MachineConfig {
            machine_name: "Test FRAMA".into(),
            job_input_dir,
            machine_output_dir: machine_input_dir.with_file_name("output"),
            machine_input_dir,
            usb_gadget_mode: false,
        },
        http: HttpConfig {
            host: "127.0.0.1".into(),
            port: 0,
        },
        plat_trunk: PlatTrunkConfig {
            url: "http://localhost:3000".into(),
            api_key: String::new(),
            status_push_interval_secs: 5,
        },
        sensor: SensorConfig::default(),
    }
}

/// Start HTTP server + file watcher on a random port. Returns the bound address.
async fn start(config: Config) -> std::net::SocketAddr {
    tokio::fs::create_dir_all(&config.machine.job_input_dir)
        .await
        .unwrap();
    tokio::fs::create_dir_all(&config.machine.machine_input_dir)
        .await
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let state = new_shared_state();
    {
        state.write().await.status = MachineStatus::Idle;
    }

    let cfg = config.clone();
    let st = state.clone();
    tokio::spawn(async move {
        run_http_server(listener, &cfg, st).await.ok();
    });

    let cfg = config.clone();
    let st = state.clone();
    tokio::spawn(async move {
        run_job_watcher(cfg.machine, st).await.ok();
    });

    sleep(Duration::from_millis(200)).await; // let server and watcher start
    addr
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// Dashboard auto-refresh: GET /health
#[tokio::test]
async fn health_check() {
    let (input, machine) = test_dirs();
    let addr = start(make_config(input, machine)).await;

    let resp = reqwest::get(format!("http://{addr}/health")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("\"ok\":true"));
}

/// Dashboard auto-refresh: GET /status returns expected fields
#[tokio::test]
async fn status_returns_expected_fields() {
    let (input, machine) = test_dirs();
    let addr = start(make_config(input, machine)).await;

    let resp = reqwest::get(format!("http://{addr}/status")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert!(json.get("status").is_some());
    assert!(json.get("queue_depth").is_some());
    assert!(json.get("coil_remaining").is_some());
    assert!(json.get("coil_low_alert").is_some());
}

/// Dashboard loads HTML
#[tokio::test]
async fn dashboard_serves_html() {
    let (input, machine) = test_dirs();
    let addr = start(make_config(input, machine)).await;

    let resp = reqwest::get(format!("http://{addr}/dashboard"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Howick Pipeline"));
}

/// Full pipeline:
/// User uploads → job queued → howick-frama polls → agent completes → job in completed list
#[tokio::test]
async fn upload_queue_agent_poll_complete() {
    let (input, machine) = test_dirs();
    let addr = start(make_config(input, machine)).await;

    let client = reqwest::Client::new();

    // 1. User uploads via dashboard
    client
        .post(format!("http://{addr}/upload"))
        .header("Content-Type", "text/plain")
        .header("X-Filename", "T1.csv")
        .body(T1_CSV)
        .send()
        .await
        .unwrap();

    sleep(Duration::from_millis(500)).await;

    // 2. Dashboard shows job queued
    let jobs: serde_json::Value = client
        .get(format!("http://{addr}/jobs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !jobs["queued"].as_array().unwrap().is_empty(),
        "job should be queued"
    );
    assert!(jobs["completed"].as_array().unwrap().is_empty());

    // 3. howick-frama polls for pending jobs
    let pending: serde_json::Value = client
        .get(format!("http://{addr}/api/jobs/howick/pending"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_list = pending["jobs"].as_array().unwrap();
    assert!(!job_list.is_empty(), "agent should see a pending job");
    assert_eq!(job_list[0]["frameset_name"], "T1");

    // 4. howick-frama confirms delivery
    let job_id = job_list[0]["job_id"].as_str().unwrap();
    let done = client
        .post(format!("http://{addr}/api/jobs/howick/{job_id}/complete"))
        .send()
        .await
        .unwrap();
    assert_eq!(done.status(), 200);

    // 5. Dashboard shows job completed, queue empty. The server flushes state
    // asynchronously, so poll (up to 5s) rather than sleep a fixed amount.
    let mut jobs = serde_json::Value::Null;
    for _ in 0..50 {
        jobs = client
            .get(format!("http://{addr}/jobs"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if jobs["queued"].as_array().unwrap().is_empty() {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(
        jobs["queued"].as_array().unwrap().is_empty(),
        "queue should be empty, got {jobs}"
    );
    assert!(
        !jobs["completed"].as_array().unwrap().is_empty(),
        "job should be completed"
    );
    assert_eq!(jobs["completed"][0]["frameset_name"], "T1");
}

/// Phase 2: coil sensor pushes weight → dashboard shows metres remaining + low alert
#[tokio::test]
async fn sensor_push_updates_coil_status() {
    let (input, machine) = test_dirs();
    let addr = start(make_config(input, machine)).await;

    let client = reqwest::Client::new();

    // Pi Zero pushes weight (23.5kg total, 18kg empty spool = 5.5kg steel / 0.74 kg/m ≈ 7.4m)
    let resp = client
        .post(format!("http://{addr}/api/sensor/coil"))
        .header("Content-Type", "application/json")
        .body(r#"{"weight_kg":23.5}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Dashboard status shows coil reading
    let status: serde_json::Value = client
        .get(format!("http://{addr}/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let coil = status["coil_remaining"].as_f64().unwrap();
    assert!(
        (coil - 7.43).abs() < 0.1,
        "coil_remaining ≈ 7.4m, got {coil}"
    );
    assert_eq!(
        status["coil_low_alert"], true,
        "7.4m < 50m threshold → low alert"
    );
}
