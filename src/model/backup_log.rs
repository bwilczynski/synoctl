//! Hyper Backup log entries, for `backups logs`.

use jiff::Timestamp;
use serde::Serialize;

/// How serious an entry is. Ordered: `Info < Warning < Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

impl LogLevel {
    /// DSM's `info`, `warn` and `err`; anything else counts as info.
    pub fn from_dsm(s: &str) -> Self {
        match s {
            "warn" => Self::Warning,
            "err" => Self::Error,
            _ => Self::Info,
        }
    }
}

/// The backup task an entry belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogTask {
    /// DSM's numeric task ID, as in `backups list`.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackupLogEntry {
    pub time: Timestamp,
    pub level: LogLevel,
    /// `None` when the entry names no known task.
    pub task: Option<LogTask>,
    pub user: String,
    /// The message without the task prefix it was matched by.
    pub message: String,
}
