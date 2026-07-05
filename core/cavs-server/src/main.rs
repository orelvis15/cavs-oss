//! `cavs-server` — stateful CAVS-1 origin.
//!
//! Serves one or more `.cavs` files over HTTP:
//!
//! - Control plane (JSON): asset list, manifests, session open.
//! - Data plane (binary `CVSP` batches): per-session inline/ref planning
//!   against the client's have-set — the session-aware dedup layer.
//! - Content-addressable chunk endpoint: stable, edge-cacheable objects.
//! - HLS passthrough: reconstructed `media.m3u8` / `init.mp4` / `seg_*.m4s`
//!   so any standard player (ffplay, Safari, VLC, hls.js) can stream
//!   directly from the origin.
//! - Prometheus-style `/metrics`.

mod state;

use anyhow::{Context, Result};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use state::{AppState, SharedState};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "cavs-server",
    version,
    about = "CAVS-1 streaming origin server"
)]
struct Cli {
    /// .cavs files to serve (asset name = file stem). Omit when using --store.
    assets: Vec<PathBuf>,
    /// Serve every asset from a shared global content-addressable store
    /// (chunks deduplicated across all assets and versions) instead of from
    /// individual .cavs files. Populate it with `cavs store <dir> add ...`.
    #[arg(long)]
    store: Option<PathBuf>,
    /// Listen address (port 0 picks a free port).
    #[arg(long, default_value = "127.0.0.1:8990")]
    listen: String,
    /// Collapse threshold: if a segment has more cold chunks than this,
    /// deliver it fully inline as a self-sufficient bundle (0 = disabled).
    #[arg(long, default_value_t = 0)]
    max_cold: usize,
    /// Path to the compiled cavs-web WASM module served at /web/cavs_web.wasm.
    #[arg(
        long,
        default_value = "target/wasm32-unknown-unknown/release/cavs_web.wasm"
    )]
    web_wasm: PathBuf,
    /// Serve HTTPS using this PEM certificate (requires --tls-key).
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// PEM private key for --tls-cert.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// Serve HTTPS with a self-signed certificate generated into this
    /// directory (cert.pem / key.pem; reused if already present). For
    /// development: point clients at cert.pem via --ca.
    #[arg(long, conflicts_with = "tls_cert")]
    tls_self_signed: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Two rustls crypto providers exist in the dependency tree; pick one
    // explicitly (idempotent — ignore if something already installed one).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    let state = Arc::new(match &cli.store {
        Some(dir) => AppState::load_store(dir, cli.max_cold, cli.web_wasm)?,
        None => {
            if cli.assets.is_empty() {
                anyhow::bail!("pass .cavs files, or --store <dir> to serve from a global store");
            }
            AppState::load(&cli.assets, cli.max_cold, cli.web_wasm)?
        }
    });
    for name in state.asset_names() {
        eprintln!("[server] asset loaded: {name}");
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/api/assets", get(list_assets))
        .route("/api/assets/{asset}/manifest", get(manifest))
        .route("/api/assets/{asset}/sessions", post(open_session))
        .route("/api/assets/{asset}/bootstrap", get(get_bootstrap))
        .route("/api/assets/{asset}/chunks/{hash}", get(get_chunk))
        .route("/api/sessions/{session}/batch", post(batch))
        .route("/hls/{asset}/{track}/{file}", get(hls_file))
        .route("/web", get(web_index))
        .route("/web/player.js", get(web_js))
        .route("/web/cavs_web.wasm", get(web_wasm))
        .route("/metrics", get(metrics))
        .with_state(state);

    let tls_files: Option<(PathBuf, PathBuf)> = match (&cli.tls_cert, &cli.tls_self_signed) {
        (Some(cert), _) => Some((cert.clone(), cli.tls_key.clone().unwrap())),
        (None, Some(dir)) => Some(ensure_self_signed(dir)?),
        (None, None) => None,
    };

    // Bind with std first so port 0 resolves before printing the banner
    // (tests and scripts parse the "listening on" line).
    let listener = std::net::TcpListener::bind(&cli.listen)
        .with_context(|| format!("cannot bind {}", cli.listen))?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;

    match tls_files {
        Some((cert, key)) => {
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .with_context(|| {
                    format!(
                        "loading TLS cert {} / key {}",
                        cert.display(),
                        key.display()
                    )
                })?;
            println!("listening on https://{addr}");
            axum_server::from_tcp_rustls(listener, config)
                .serve(app.into_make_service())
                .await?;
        }
        None => {
            println!("listening on http://{addr}");
            axum::serve(tokio::net::TcpListener::from_std(listener)?, app).await?;
        }
    }
    Ok(())
}

/// Generate (or reuse) a self-signed localhost certificate for development.
fn ensure_self_signed(dir: &PathBuf) -> Result<(PathBuf, PathBuf)> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    if !cert_path.exists() || !key_path.exists() {
        std::fs::create_dir_all(dir)?;
        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
        ])
        .context("generating self-signed certificate")?;
        std::fs::write(&cert_path, cert.cert.pem())?;
        std::fs::write(&key_path, cert.key_pair.serialize_pem())?;
        eprintln!(
            "[server] self-signed TLS cert written to {}",
            cert_path.display()
        );
    }
    Ok((cert_path, key_path))
}

type AppError = (StatusCode, String);

fn not_found(what: impl std::fmt::Display) -> AppError {
    (StatusCode::NOT_FOUND, format!("{what} not found"))
}

async fn index(State(state): State<SharedState>) -> Html<String> {
    let mut html = String::from(
        "<h1>cavs-server</h1><p><a href=\"/web\">reproductor web (WASM + MSE)</a></p><ul>",
    );
    for name in state.asset_names() {
        html.push_str(&format!(
            "<li>{name} — <a href=\"/api/assets/{name}/manifest\">manifest</a>"
        ));
        for track in state.video_track_names(&name) {
            html.push_str(&format!(
                " | <a href=\"/hls/{name}/{track}/media.m3u8\">hls:{track}</a>"
            ));
        }
        html.push_str("</li>");
    }
    html.push_str("</ul>");
    Html(html)
}

async fn list_assets(State(state): State<SharedState>) -> Json<Vec<cavs_proto::AssetSummary>> {
    Json(state.summaries())
}

async fn manifest(
    State(state): State<SharedState>,
    Path(asset): Path<String>,
) -> Result<Json<cavs_proto::Manifest>, AppError> {
    state
        .manifest(&asset)
        .map(Json)
        .ok_or_else(|| not_found(format!("asset {asset}")))
}

async fn open_session(
    State(state): State<SharedState>,
    Path(asset): Path<String>,
    Json(req): Json<cavs_proto::SessionOpenRequest>,
) -> Result<Json<cavs_proto::SessionOpenResponse>, AppError> {
    state
        .open_session(&asset, &req.have, req.have_bloom.as_ref())
        .map(Json)
        .ok_or_else(|| not_found(format!("asset {asset}")))
}

async fn batch(
    State(state): State<SharedState>,
    Path(session): Path<String>,
    Json(req): Json<cavs_proto::BatchRequest>,
) -> Result<Response, AppError> {
    let bytes = state
        .plan_batch(&session, &req)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

/// Full bootstrap artifact (whole asset, zstd): the cold-install fast path.
/// Streamed from disk so a multi-hundred-MiB artifact never sits in RAM.
async fn get_bootstrap(
    State(state): State<SharedState>,
    Path(asset): Path<String>,
) -> Result<Response, AppError> {
    let (path, size) = state
        .bootstrap_file(&asset)
        .ok_or_else(|| not_found(format!("bootstrap for {asset}")))?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let stream = tokio_util::io::ReaderStream::new(file);
    Ok((
        [
            (header::CONTENT_TYPE, "application/zstd".to_string()),
            (header::CONTENT_LENGTH, size.to_string()),
            // Tied to the packed content: immutable, edge-cacheable.
            (
                header::CACHE_CONTROL,
                "public, max-age=31536000, immutable".to_string(),
            ),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

async fn get_chunk(
    State(state): State<SharedState>,
    Path((asset, hash)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let bytes = state
        .chunk_by_hash(&asset, &hash)
        .ok_or_else(|| not_found(format!("chunk {hash}")))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            // Content-addressed: immutable forever, ideal for edge caches.
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response())
}

async fn hls_file(
    State(state): State<SharedState>,
    Path((asset, track, file)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    let (bytes, content_type) = state
        .hls_file(&asset, &track, &file)
        .ok_or_else(|| not_found(format!("{asset}/{track}/{file}")))?;
    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

async fn web_index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn web_js() -> Response {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("../static/player.js"),
    )
        .into_response()
}

async fn web_wasm(State(state): State<SharedState>) -> Result<Response, AppError> {
    let path = state.web_wasm_path();
    // Small module read on demand; std read keeps tokio features minimal.
    let bytes = std::fs::read(path).map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            format!(
                "WASM module not found at {}. Build it with:\n  cargo build -p cavs-web \
                 --target wasm32-unknown-unknown --release\nor pass --web-wasm <path>.",
                path.display()
            ),
        )
    })?;
    Ok(([(header::CONTENT_TYPE, "application/wasm")], bytes).into_response())
}

async fn metrics(State(state): State<SharedState>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.render_metrics(),
    )
        .into_response()
}
