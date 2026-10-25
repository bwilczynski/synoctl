//! `backups logs`: journalctl's `short` format, with the task name where
//! journalctl shows the identifier, and JSON Lines.

use jiff::tz::TimeZone;
use serde::Serialize;

use super::style::{Paint, paint};
use crate::error::{Error, Result};
use crate::model::backup_log::{BackupLogEntry, LogLevel, LogTask};

/// `Oct 25 01:16:42 Backup to BUFFALO: <message>`, the message coloured by
/// level when `color`.
pub fn line(e: &BackupLogEntry, tz: &TimeZone, color: bool) -> String {
    let time = e.time.to_zoned(tz.clone()).strftime("%b %d %H:%M:%S");
    let task = e.task.as_ref().map_or("-", |t| t.name.as_str());
    let message = match e.level {
        LogLevel::Error => paint(&e.message, Paint::Red, color),
        LogLevel::Warning => paint(&e.message, Paint::Yellow, color),
        LogLevel::Info => e.message.clone(),
    };
    format!("{time} {task}: {message}\n")
}

#[derive(Serialize)]
struct Line<'a> {
    time: String,
    level: LogLevel,
    task: Option<&'a LogTask>,
    user: &'a str,
    message: &'a str,
}

/// One compact JSON object and a newline; `time` in `tz` with its offset.
pub fn json_line(e: &BackupLogEntry, tz: &TimeZone) -> Result<String> {
    let line = Line {
        time: e
            .time
            .to_zoned(tz.clone())
            .strftime("%Y-%m-%dT%H:%M:%S%:z")
            .to_string(),
        level: e.level,
        task: e.task.as_ref(),
        user: &e.user,
        message: &e.message,
    };
    serde_json::to_string(&line)
        .map(|s| s + "\n")
        .map_err(|err| Error::Other(format!("encode JSON: {err}")))
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::*;
    use crate::model::backup_log::{BackupLogEntry, LogLevel, LogTask};
    use crate::render::style::{Paint, paint};

    fn warsaw() -> TimeZone {
        TimeZone::get("Europe/Warsaw").unwrap()
    }

    fn entry(level: LogLevel, task: bool) -> BackupLogEntry {
        BackupLogEntry {
            time: "2026-10-24T23:16:42Z".parse().unwrap(),
            level,
            task: task.then(|| LogTask {
                id: "3".into(),
                name: "Backup to BUFFALO".into(),
            }),
            user: "SYSTEM".into(),
            message: "Incomplete file backup.".into(),
        }
    }

    #[test]
    fn line_shows_time_task_and_message() {
        assert_eq!(
            line(&entry(LogLevel::Warning, true), &warsaw(), false),
            "Oct 25 01:16:42 Backup to BUFFALO: Incomplete file backup.\n"
        );
    }

    #[test]
    fn line_without_task_uses_dash() {
        assert_eq!(
            line(&entry(LogLevel::Info, false), &warsaw(), false),
            "Oct 25 01:16:42 -: Incomplete file backup.\n"
        );
    }

    #[test]
    fn line_colours_message_by_level() {
        let msg = "Incomplete file backup.";
        let prefix = "Oct 25 01:16:42 Backup to BUFFALO: ";
        for (level, want) in [
            (LogLevel::Error, paint(msg, Paint::Red, true)),
            (LogLevel::Warning, paint(msg, Paint::Yellow, true)),
            (LogLevel::Info, msg.to_string()),
        ] {
            assert_eq!(
                line(&entry(level, true), &warsaw(), true),
                format!("{prefix}{want}\n")
            );
        }
    }

    #[test]
    fn no_escapes_without_colour() {
        for level in [LogLevel::Info, LogLevel::Warning, LogLevel::Error] {
            assert!(!line(&entry(level, true), &warsaw(), false).contains('\x1b'));
        }
    }

    #[test]
    fn json_line_shape() {
        assert_eq!(
            json_line(&entry(LogLevel::Warning, true), &warsaw()).unwrap(),
            "{\"time\":\"2026-10-25T01:16:42+02:00\",\"level\":\"warning\",\
             \"task\":{\"id\":\"3\",\"name\":\"Backup to BUFFALO\"},\"user\":\"SYSTEM\",\
             \"message\":\"Incomplete file backup.\"}\n"
        );
        assert!(
            json_line(&entry(LogLevel::Info, false), &warsaw())
                .unwrap()
                .contains("\"task\":null")
        );
    }
}
