//! Tauri command surface consumed by the React frontend.
//!
//! Everything the UI needs funnels through here: settings, per-section
//! history (list/get/delete), running CAVS operations against the shared Rust
//! core, external-tool detection and the local dev server.

use crate::appstate::AppState;
use crate::db::{self, OperationRecord, Project};
use crate::error::DesktopError;
use crate::{server, storage};
use cavs_sdk_core::ProgressEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use tauri::{AppHandle, Emitter, State};

// ---------------------------------------------------------------------------
// App info & settings
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    app_version: String,
    sdk_version: String,
    abi_version: String,
    os: String,
    arch: String,
    operations: Vec<String>,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        sdk_version: cavs_sdk_core::SDK_VERSION.to_string(),
        abi_version: cavs_sdk_core::ABI_VERSION.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        operations: cavs_sdk_core::OPERATIONS.iter().map(|s| s.to_string()).collect(),
    }
}

const SETTINGS_KEY: &str = "app";

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Result<Value, DesktopError> {
    let conn = state.db.lock().unwrap();
    let raw = db::get_setting(&conn, SETTINGS_KEY)?;
    let parsed = raw.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(parsed.unwrap_or_else(default_settings))
}

#[tauri::command]
pub fn save_settings(state: State<AppState>, settings: Value) -> Result<Value, DesktopError> {
    let conn = state.db.lock().unwrap();
    db::set_setting(&conn, SETTINGS_KEY, &settings.to_string())?;
    Ok(settings)
}

fn default_settings() -> Value {
    json!({
        "language": "es",
        "theme": "dark",
        "defaultOutputFolder": null,
        "localServerPort": 8990,
        "showCliPreview": true,
        "recentProjectsLimit": 10
    })
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProject {
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default = "default_engine")]
    pub engine: String,
    pub output_folder: String,
}

fn default_engine() -> String {
    "godot".to_string()
}

#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<Vec<Project>, DesktopError> {
    let conn = state.db.lock().unwrap();
    db::list_projects(&conn)
}

#[tauri::command]
pub fn create_project(
    state: State<AppState>,
    project: NewProject,
) -> Result<Project, DesktopError> {
    let name = project.name.trim().to_string();
    let folder = project.output_folder.trim().to_string();
    if name.is_empty() {
        return Err(DesktopError::bad_request("Project name is required."));
    }
    if folder.is_empty() {
        return Err(DesktopError::bad_request("Output folder is required."));
    }
    storage::ensure_dir(Path::new(&folder))?;

    let now = chrono::Local::now().to_rfc3339();
    let rec = Project {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        icon: project.icon.filter(|s| !s.is_empty()),
        engine: project.engine,
        output_folder: folder,
        created_at: now.clone(),
        updated_at: now,
    };
    let conn = state.db.lock().unwrap();
    db::insert_project(&conn, &rec)?;
    Ok(rec)
}

#[tauri::command]
pub fn update_project(
    state: State<AppState>,
    project: Project,
) -> Result<Project, DesktopError> {
    if project.name.trim().is_empty() {
        return Err(DesktopError::bad_request("Project name is required."));
    }
    if project.output_folder.trim().is_empty() {
        return Err(DesktopError::bad_request("Output folder is required."));
    }
    storage::ensure_dir(Path::new(&project.output_folder))?;
    let mut updated = project;
    updated.updated_at = chrono::Local::now().to_rfc3339();
    updated.icon = updated.icon.filter(|s| !s.is_empty());
    let conn = state.db.lock().unwrap();
    db::update_project(&conn, &updated)?;
    Ok(updated)
}

#[tauri::command]
pub fn delete_project(state: State<AppState>, id: String) -> Result<(), DesktopError> {
    let conn = state.db.lock().unwrap();
    // Remove the generated artifact folders for this project's operations, but
    // never the project's root output folder (it may be user-owned).
    for op in db::list_project_operations(&conn, &id)? {
        storage::remove_dir_all(Path::new(&op.artifact_dir))?;
    }
    db::delete_project(&conn, &id)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_operations(
    state: State<AppState>,
    project_id: String,
    section: String,
) -> Result<Vec<OperationRecord>, DesktopError> {
    let conn = state.db.lock().unwrap();
    db::list_operations(&conn, &project_id, &section)
}

#[tauri::command]
pub fn list_project_operations(
    state: State<AppState>,
    project_id: String,
) -> Result<Vec<OperationRecord>, DesktopError> {
    let conn = state.db.lock().unwrap();
    db::list_project_operations(&conn, &project_id)
}

#[tauri::command]
pub fn get_operation(
    state: State<AppState>,
    id: String,
) -> Result<Option<OperationRecord>, DesktopError> {
    let conn = state.db.lock().unwrap();
    db::get_operation(&conn, &id)
}

/// Delete a history entry *and* its generated files on disk (spec: "eliminar
/// esto eliminaría los archivos también").
#[tauri::command]
pub fn delete_operation(state: State<AppState>, id: String) -> Result<(), DesktopError> {
    let conn = state.db.lock().unwrap();
    if let Some(rec) = db::get_operation(&conn, &id)? {
        storage::remove_dir_all(Path::new(&rec.artifact_dir))?;
    }
    db::delete_operation(&conn, &id)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Run operation
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    pub project_id: String,
    pub section: String,
    /// A CAVS core operation name (e.g. `analyze`, `packDirectory`,
    /// `createPlan`, `applyPlan`, `verifyInstall`, `previewUpdate`,
    /// `benchmark`, `estimateSavings`).
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub params: Value,
}

/// Param keys whose value is an output path we want to land inside the
/// per-operation artifact directory when the caller passes a bare filename.
const OUTPUT_KEYS: &[&str] = &["outputCavs", "outputPlan", "outputPath", "output"];

#[tauri::command]
pub async fn run_operation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunRequest,
) -> Result<OperationRecord, DesktopError> {
    let RunRequest {
        project_id,
        section,
        kind,
        title,
        mut params,
    } = request;

    if !cavs_sdk_core::OPERATIONS.contains(&kind.as_str()) {
        return Err(DesktopError::bad_request(&format!(
            "Unknown operation '{kind}'."
        )));
    }

    // Resolve the project so artifacts land under its own output folder.
    let project = {
        let conn = state.db.lock().unwrap();
        db::get_project(&conn, &project_id)?
    }
    .ok_or_else(|| {
        DesktopError::new(
            "DESKTOP-E-NO-PROJECT",
            "Project not found",
            "This operation is not associated with a valid project.",
        )
    })?;

    let base = std::path::PathBuf::from(&project.output_folder);
    storage::ensure_dir(&base)?;

    let op_id = uuid::Uuid::new_v4().to_string();
    let dir = storage::operation_dir(&base, &section, &op_id)?;

    // Redirect bare output filenames into the operation's own folder.
    if let Some(obj) = params.as_object_mut() {
        for key in OUTPUT_KEYS {
            if let Some(Value::String(name)) = obj.get(*key) {
                let p = Path::new(name);
                if p.is_relative() {
                    let joined = dir.join(name);
                    obj.insert(
                        (*key).to_string(),
                        Value::String(joined.to_string_lossy().to_string()),
                    );
                }
            }
        }
    }

    // Emit progress events to the frontend as `cavs://progress`.
    let app_for_sink = app.clone();
    let op_for_sink = op_id.clone();
    let sink = move |e: &ProgressEvent| {
        let payload = json!({
            "opId": op_for_sink,
            "event": serde_json::to_value(e).unwrap_or(Value::Null),
        });
        let _ = app_for_sink.emit("cavs://progress", payload);
    };

    let params_for_run = params.clone();
    let kind_for_run = kind.clone();
    // Run the (potentially long) CAVS operation off the async worker so we do
    // not hold the DB lock while it executes.
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let sink_ref: cavs_sdk_core::ProgressSink = &sink;
        cavs_sdk_core::dispatch(&kind_for_run, &params_for_run, Some(sink_ref), None)
    })
    .await
    .map_err(|e| {
        DesktopError::new(
            "DESKTOP-E-JOIN",
            "Task failed",
            "A background task did not complete.",
        )
        .with_technical(e)
    })?;

    let now = chrono::Local::now().to_rfc3339();
    let (status, result, error) = match outcome {
        Ok(data) => ("completed".to_string(), data, None),
        Err(e) => (
            "failed".to_string(),
            Value::Null,
            Some(json!({
                "code": e.code(),
                "message": e.to_string(),
                "recoverable": e.recoverable(),
            })),
        ),
    };

    // Capture generated files (everything the op wrote, before our metadata).
    let files = list_dir_files(&dir);

    // Persist inputs + result next to the artifacts for export / open-folder.
    let _ = std::fs::write(
        dir.join("params.json"),
        serde_json::to_vec_pretty(&params).unwrap_or_default(),
    );
    let record_payload = if error.is_some() {
        json!({ "error": error })
    } else {
        result.clone()
    };
    let _ = std::fs::write(
        dir.join("result.json"),
        serde_json::to_vec_pretty(&record_payload).unwrap_or_default(),
    );

    let rec = OperationRecord {
        id: op_id,
        project_id,
        section,
        kind,
        title,
        status,
        created_at: now,
        params,
        result,
        artifact_dir: dir.to_string_lossy().to_string(),
        error,
        files,
    };

    {
        let conn = state.db.lock().unwrap();
        db::insert_operation(&conn, &rec)?;
    }
    Ok(rec)
}

fn list_dir_files(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.path().is_file() {
                if let Some(name) = entry.file_name().to_str() {
                    if name != "params.json" && name != "result.json" {
                        names.push(name.to_string());
                    }
                }
            }
        }
    }
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// Open path in OS file manager
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn open_path(path: String) -> Result<(), DesktopError> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(DesktopError::new(
            "DESKTOP-E-NOT-FOUND",
            "Path not found",
            "The file or folder no longer exists.",
        )
        .with_actions(&["It may have been deleted or moved."]));
    }
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    std::process::Command::new(program)
        .arg(&path)
        .spawn()
        .map_err(|e| DesktopError::io("open the path", e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// External tool detection (spec §28.3, §42.6)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    name: String,
    available: bool,
    path: Option<String>,
    version: Option<String>,
}

/// Detect external tools. Runs off the main thread (`spawn_blocking`) because
/// it shells out to up to six subprocesses; a synchronous command would block
/// the UI event loop.
#[tauri::command]
pub async fn detect_tools() -> Vec<ToolStatus> {
    tauri::async_runtime::spawn_blocking(|| {
        let tools: &[(&str, &[&str])] = &[
            ("butler", &["version"]),
            ("bsdiff", &["--help"]),
            ("xdelta3", &["-V"]),
            ("zstd", &["--version"]),
            ("brotli", &["--version"]),
            ("godot", &["--version"]),
        ];
        tools
            .iter()
            .map(|(name, args)| detect_one(name, args))
            .collect()
    })
    .await
    .unwrap_or_default()
}

fn detect_one(name: &str, args: &[&str]) -> ToolStatus {
    let path = which(name);
    let version = std::process::Command::new(name)
        .args(args)
        .output()
        .ok()
        .and_then(|out| {
            let text = if !out.stdout.is_empty() {
                String::from_utf8_lossy(&out.stdout).to_string()
            } else {
                String::from_utf8_lossy(&out.stderr).to_string()
            };
            text.lines().next().map(|l| l.trim().to_string())
        })
        .filter(|s| !s.is_empty());
    ToolStatus {
        name: name.to_string(),
        available: path.is_some(),
        path,
        version,
    }
}

fn which(name: &str) -> Option<String> {
    #[cfg(windows)]
    let finder = "where";
    #[cfg(not(windows))]
    let finder = "which";
    let out = std::process::Command::new(finder).arg(name).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().next().map(|l| l.trim().to_string()).filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Local dev server (spec §22)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn server_start(
    state: State<'_, AppState>,
    dir: String,
    port: u16,
) -> Result<server::ServerStatus, DesktopError> {
    // Stop any existing server first.
    if let Some(existing) = state.server.lock().unwrap().take() {
        existing.stop();
    }
    if !Path::new(&dir).is_dir() {
        return Err(DesktopError::new(
            "DESKTOP-E-NO-DIR",
            "Folder not found",
            "Select a valid workspace or release folder to serve.",
        ));
    }
    let running = server::start(dir, port)
        .await
        .map_err(|e| {
            DesktopError::new("DESKTOP-E-SERVER", "Server error", &e)
                .with_actions(&["Try a different port.", "Check that the port is free."])
                .recoverable()
        })?;
    let status = running.status();
    *state.server.lock().unwrap() = Some(running);
    Ok(status)
}

#[tauri::command]
pub fn server_stop(state: State<AppState>) -> server::ServerStatus {
    if let Some(existing) = state.server.lock().unwrap().take() {
        existing.stop();
    }
    server::ServerStatus::stopped()
}

#[tauri::command]
pub fn server_status(state: State<AppState>) -> server::ServerStatus {
    match state.server.lock().unwrap().as_ref() {
        Some(s) => s.status(),
        None => server::ServerStatus::stopped(),
    }
}

#[tauri::command]
pub fn server_logs(state: State<AppState>) -> Vec<server::RequestLog> {
    match state.server.lock().unwrap().as_ref() {
        Some(s) => s.logs.lock().unwrap().clone(),
        None => Vec::new(),
    }
}
