//! Local app-data layout (spec §4.4).
//!
//! App-level metadata lives under `~/.cavs-desktop` (the SQLite DB). Generated
//! artifacts live inside each **project's own output folder**, organized by
//! section and operation:
//!
//! ```text
//! ~/.cavs-desktop/cavs-desktop.db      (SQLite — projects, history, settings)
//!
//! <project.output_folder>/
//!   {section}/{operation_id}/          (generated files, next to their record)
//! ```

use crate::error::DesktopError;
use std::path::{Path, PathBuf};

/// Root of all CAVS Desktop app-level data: `~/.cavs-desktop`.
pub fn app_root() -> Result<PathBuf, DesktopError> {
    let home = dirs::home_dir().ok_or_else(|| {
        DesktopError::new(
            "DESKTOP-E-NO-HOME",
            "No home directory",
            "Could not determine your home directory.",
        )
    })?;
    let root = home.join(".cavs-desktop");
    ensure_dir(&root)?;
    Ok(root)
}

pub fn db_path() -> Result<PathBuf, DesktopError> {
    Ok(app_root()?.join("cavs-desktop.db"))
}

/// Per-operation artifact directory inside the project folder:
/// `<base>/{section}/{operation_id}`. Matches the spec's
/// `/patch/{operation_id}` example.
pub fn operation_dir(
    base: &Path,
    section: &str,
    operation_id: &str,
) -> Result<PathBuf, DesktopError> {
    let dir = base.join(sanitize(section)).join(sanitize(operation_id));
    ensure_dir(&dir)?;
    Ok(dir)
}

pub fn ensure_dir(path: &Path) -> Result<(), DesktopError> {
    std::fs::create_dir_all(path)
        .map_err(|e| DesktopError::io(&format!("create directory {}", path.display()), e))
}

/// Recursively delete an operation's artifact directory. Missing is fine.
pub fn remove_dir_all(path: &Path) -> Result<(), DesktopError> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .map_err(|e| DesktopError::io(&format!("delete directory {}", path.display()), e))?;
    }
    Ok(())
}

/// Guard against path traversal in section / id components.
fn sanitize(component: &str) -> String {
    component
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
