//! SQLite persistence: projects, per-section operation history and settings.

use crate::error::DesktopError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A local CAVS Desktop project (spec §7, §43.1). Everything a user does is
/// scoped to a project and stored under its output folder.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub engine: String,
    pub output_folder: String,
    pub created_at: String,
    pub updated_at: String,
}

/// One entry in a section's history table (spec §5, §25, §26).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub id: String,
    pub project_id: String,
    pub section: String,
    pub kind: String,
    pub title: String,
    pub status: String, // "completed" | "failed"
    pub created_at: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub result: Value,
    pub artifact_dir: String,
    #[serde(default)]
    pub error: Option<Value>,
    /// Names of generated files inside `artifact_dir` (for the "open folder" UX).
    #[serde(default)]
    pub files: Vec<String>,
}

pub fn open() -> Result<Connection, DesktopError> {
    let path = crate::storage::db_path()?;
    let conn = Connection::open(&path).map_err(DesktopError::db)?;
    init(&conn)?;
    Ok(conn)
}

fn init(conn: &Connection) -> Result<(), DesktopError> {
    // Create tables first. Note: no index that references project_id yet — a
    // database created before projects existed will not have that column until
    // the migration below runs.
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS projects (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            icon          TEXT,
            engine        TEXT NOT NULL,
            output_folder TEXT NOT NULL,
            created_at    TEXT NOT NULL,
            updated_at    TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS operations (
            id           TEXT PRIMARY KEY,
            project_id   TEXT NOT NULL DEFAULT '',
            section      TEXT NOT NULL,
            kind         TEXT NOT NULL,
            title        TEXT NOT NULL,
            status       TEXT NOT NULL,
            created_at   TEXT NOT NULL,
            params       TEXT NOT NULL,
            result       TEXT NOT NULL,
            error        TEXT,
            artifact_dir TEXT NOT NULL,
            files        TEXT NOT NULL DEFAULT '[]'
        );

        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )
    .map_err(DesktopError::db)?;

    // Migration: add project_id to pre-existing `operations` tables. Ignored
    // (duplicate-column error) when the column already exists.
    let _ = conn.execute(
        "ALTER TABLE operations ADD COLUMN project_id TEXT NOT NULL DEFAULT ''",
        [],
    );

    // Now the column is guaranteed to exist — safe to index it.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_operations_project_section
             ON operations(project_id, section, created_at DESC)",
        [],
    )
    .map_err(DesktopError::db)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

fn row_to_project(row: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        icon: row.get("icon")?,
        engine: row.get("engine")?,
        output_folder: row.get("output_folder")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub fn insert_project(conn: &Connection, p: &Project) -> Result<(), DesktopError> {
    conn.execute(
        "INSERT INTO projects (id, name, icon, engine, output_folder, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![p.id, p.name, p.icon, p.engine, p.output_folder, p.created_at, p.updated_at],
    )?;
    Ok(())
}

pub fn update_project(conn: &Connection, p: &Project) -> Result<(), DesktopError> {
    conn.execute(
        "UPDATE projects SET name=?2, icon=?3, engine=?4, output_folder=?5, updated_at=?6
         WHERE id=?1",
        params![p.id, p.name, p.icon, p.engine, p.output_folder, p.updated_at],
    )?;
    Ok(())
}

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>, DesktopError> {
    let mut stmt = conn.prepare("SELECT * FROM projects ORDER BY updated_at DESC")?;
    let rows = stmt
        .query_map([], row_to_project)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_project(conn: &Connection, id: &str) -> Result<Option<Project>, DesktopError> {
    let mut stmt = conn.prepare("SELECT * FROM projects WHERE id = ?1")?;
    Ok(stmt.query_row(params![id], row_to_project).optional()?)
}

pub fn delete_project(conn: &Connection, id: &str) -> Result<(), DesktopError> {
    conn.execute("DELETE FROM operations WHERE project_id = ?1", params![id])?;
    conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

pub fn insert_operation(conn: &Connection, rec: &OperationRecord) -> Result<(), DesktopError> {
    conn.execute(
        "INSERT INTO operations
            (id, project_id, section, kind, title, status, created_at, params, result, error, artifact_dir, files)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            rec.id,
            rec.project_id,
            rec.section,
            rec.kind,
            rec.title,
            rec.status,
            rec.created_at,
            rec.params.to_string(),
            rec.result.to_string(),
            rec.error.as_ref().map(|e| e.to_string()),
            rec.artifact_dir,
            serde_json::to_string(&rec.files).unwrap_or_else(|_| "[]".into()),
        ],
    )?;
    Ok(())
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<OperationRecord> {
    let params_s: String = row.get("params")?;
    let result_s: String = row.get("result")?;
    let error_s: Option<String> = row.get("error")?;
    let files_s: String = row.get("files")?;
    Ok(OperationRecord {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        section: row.get("section")?,
        kind: row.get("kind")?,
        title: row.get("title")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        params: serde_json::from_str(&params_s).unwrap_or(Value::Null),
        result: serde_json::from_str(&result_s).unwrap_or(Value::Null),
        error: error_s.and_then(|s| serde_json::from_str(&s).ok()),
        artifact_dir: row.get("artifact_dir")?,
        files: serde_json::from_str(&files_s).unwrap_or_default(),
    })
}

pub fn list_operations(
    conn: &Connection,
    project_id: &str,
    section: &str,
) -> Result<Vec<OperationRecord>, DesktopError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM operations WHERE project_id = ?1 AND section = ?2
         ORDER BY created_at DESC, rowid DESC",
    )?;
    let rows = stmt
        .query_map(params![project_id, section], row_to_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn list_project_operations(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<OperationRecord>, DesktopError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM operations WHERE project_id = ?1 ORDER BY created_at DESC, rowid DESC",
    )?;
    let rows = stmt
        .query_map(params![project_id], row_to_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_operation(conn: &Connection, id: &str) -> Result<Option<OperationRecord>, DesktopError> {
    let mut stmt = conn.prepare("SELECT * FROM operations WHERE id = ?1")?;
    Ok(stmt.query_row(params![id], row_to_record).optional()?)
}

pub fn delete_operation(conn: &Connection, id: &str) -> Result<(), DesktopError> {
    conn.execute("DELETE FROM operations WHERE id = ?1", params![id])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>, DesktopError> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    Ok(stmt
        .query_row(params![key], |r| r.get::<_, String>(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<(), DesktopError> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}
