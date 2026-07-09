//! UI-friendly error type shared by all Tauri commands.
//!
//! Every command returns `Result<T, DesktopError>`. The error serializes to a
//! stable shape the React layer can render directly (code, title,
//! description, suggested actions, technical detail) — see spec §42.4.

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopError {
    /// Stable machine code, e.g. `CAVS-E-PLAN-INVALID` or `DESKTOP-E-IO`.
    pub code: String,
    /// Short human title.
    pub title: String,
    /// Plain-language description of what went wrong.
    pub description: String,
    /// Concrete next steps the user can try.
    pub suggested_actions: Vec<String>,
    /// Raw technical detail (kept, but shown behind a disclosure in the UI).
    pub technical: Option<String>,
    /// Whether retrying the same action might succeed.
    pub recoverable: bool,
}

impl DesktopError {
    pub fn new(code: &str, title: &str, description: &str) -> Self {
        DesktopError {
            code: code.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            suggested_actions: Vec::new(),
            technical: None,
            recoverable: false,
        }
    }

    pub fn with_actions(mut self, actions: &[&str]) -> Self {
        self.suggested_actions = actions.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn with_technical(mut self, technical: impl std::fmt::Display) -> Self {
        self.technical = Some(technical.to_string());
        self
    }

    pub fn recoverable(mut self) -> Self {
        self.recoverable = true;
        self
    }

    pub fn io(context: &str, err: impl std::fmt::Display) -> Self {
        DesktopError::new(
            "DESKTOP-E-IO",
            "File system error",
            &format!("Could not {context}."),
        )
        .with_technical(err)
        .with_actions(&[
            "Check that the file or folder exists and is readable.",
            "Check that you have permission to write to the output folder.",
        ])
    }

    pub fn db(err: impl std::fmt::Display) -> Self {
        DesktopError::new(
            "DESKTOP-E-DB",
            "Local database error",
            "CAVS Desktop could not read or write its local database.",
        )
        .with_technical(err)
        .with_actions(&["Restart the application.", "Check disk space and permissions."])
    }

    pub fn bad_request(msg: &str) -> Self {
        DesktopError::new("DESKTOP-E-REQUEST", "Invalid request", msg)
    }
}

impl From<rusqlite::Error> for DesktopError {
    fn from(e: rusqlite::Error) -> Self {
        DesktopError::db(e)
    }
}
