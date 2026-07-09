//! Local development test server (spec §22).
//!
//! Serves a workspace/release folder over plain HTTP for plugin testing.
//! Development only — never a production CDN. Range requests are supported so
//! Godot/CAVS clients can fetch chunks.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Default)]
pub struct ServerStats {
    pub requests: AtomicU64,
    pub bytes: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLog {
    pub time: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub bytes: u64,
    pub duration_ms: u64,
}

pub struct RunningServer {
    pub port: u16,
    pub dir: String,
    pub started_at: String,
    shutdown: Option<oneshot::Sender<()>>,
    pub stats: Arc<ServerStats>,
    pub logs: Arc<Mutex<Vec<RequestLog>>>,
    pub last_error: Arc<Mutex<Option<String>>>,
}

impl RunningServer {
    pub fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub dir: Option<String>,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub requests: u64,
    pub bytes_served: u64,
    pub last_error: Option<String>,
}

impl ServerStatus {
    pub fn stopped() -> Self {
        ServerStatus {
            running: false,
            port: None,
            dir: None,
            url: None,
            started_at: None,
            requests: 0,
            bytes_served: 0,
            last_error: None,
        }
    }
}

impl RunningServer {
    pub fn status(&self) -> ServerStatus {
        ServerStatus {
            running: true,
            port: Some(self.port),
            dir: Some(self.dir.clone()),
            url: Some(format!("http://localhost:{}", self.port)),
            started_at: Some(self.started_at.clone()),
            requests: self.stats.requests.load(Ordering::Relaxed),
            bytes_served: self.stats.bytes.load(Ordering::Relaxed),
            last_error: self.last_error.lock().unwrap().clone(),
        }
    }
}

/// Start serving `dir` on `port`. Returns immediately once the socket is bound.
pub async fn start(dir: String, port: u16) -> Result<RunningServer, String> {
    let stats = Arc::new(ServerStats::default());
    let logs: Arc<Mutex<Vec<RequestLog>>> = Arc::new(Mutex::new(Vec::new()));
    let last_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let serve_dir = tower_http::services::ServeDir::new(&dir);

    let mw_stats = stats.clone();
    let mw_logs = logs.clone();
    let app = axum::Router::new()
        .fallback_service(serve_dir)
        .layer(axum::middleware::from_fn(move |req: Request, next: Next| {
            let stats = mw_stats.clone();
            let logs = mw_logs.clone();
            async move { record(stats, logs, req, next).await }
        }));

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Could not bind to port {port}: {e}"))?;

    let (tx, rx) = oneshot::channel::<()>();
    let err_slot = last_error.clone();
    tauri::async_runtime::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = rx.await;
        });
        if let Err(e) = server.await {
            *err_slot.lock().unwrap() = Some(e.to_string());
        }
    });

    Ok(RunningServer {
        port,
        dir,
        started_at: chrono::Local::now().to_rfc3339(),
        shutdown: Some(tx),
        stats,
        logs,
        last_error,
    })
}

async fn record(
    stats: Arc<ServerStats>,
    logs: Arc<Mutex<Vec<RequestLog>>>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let start = std::time::Instant::now();
    let resp = next.run(req).await;
    let status = resp.status().as_u16();
    let bytes = resp
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    stats.requests.fetch_add(1, Ordering::Relaxed);
    stats.bytes.fetch_add(bytes, Ordering::Relaxed);

    let mut guard = logs.lock().unwrap();
    guard.push(RequestLog {
        time: chrono::Local::now().format("%H:%M:%S").to_string(),
        method,
        path,
        status,
        bytes,
        duration_ms: start.elapsed().as_millis() as u64,
    });
    // Keep only the most recent entries.
    let len = guard.len();
    if len > 500 {
        guard.drain(0..len - 500);
    }
    drop(guard);

    resp
}
