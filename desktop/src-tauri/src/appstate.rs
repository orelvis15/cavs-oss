//! Shared, thread-safe application state managed by Tauri.

use crate::db;
use crate::error::DesktopError;
use crate::server::RunningServer;
use rusqlite::Connection;
use std::sync::Mutex;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub server: Mutex<Option<RunningServer>>,
}

impl AppState {
    pub fn new() -> Result<Self, DesktopError> {
        Ok(AppState {
            db: Mutex::new(db::open()?),
            server: Mutex::new(None),
        })
    }
}
