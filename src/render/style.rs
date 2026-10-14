//! ANSI colour for status cells and health metrics. Only the basic SGR
//! colours are used, so the terminal's own palette decides the shade.

use crate::model::docker::{ContainerStatus, UpdateStatus};
use crate::model::{BackupTaskResult, BackupTaskStatus, DiskStatus, Verdict, VolumeStatus};
use crate::service::{DISK_WARN_C, VOLUME_ERROR_PCT, VOLUME_WARN_PCT};
use crate::setup::check::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Green,
    Red,
    BoldRed,
    Yellow,
    Blue,
    Dim,
}

impl Paint {
    fn sgr(self) -> &'static str {
        match self {
            Self::Green => "32",
            Self::Red => "31",
            Self::BoldRed => "1;31",
            Self::Yellow => "33",
            Self::Blue => "34",
            Self::Dim => "2",
        }
    }
}

/// `text` wrapped in `p`'s escape codes when `on`, unchanged otherwise.
pub fn paint(text: &str, p: Paint, on: bool) -> String {
    if on {
        format!("\x1b[{}m{text}\x1b[0m", p.sgr())
    } else {
        text.to_string()
    }
}

pub fn volume_status(s: VolumeStatus, on: bool) -> String {
    let p = match s {
        VolumeStatus::Normal => Paint::Green,
        VolumeStatus::Degraded | VolumeStatus::Repairing => Paint::Yellow,
        VolumeStatus::Crashed => Paint::Red,
    };
    paint(s.as_str(), p, on)
}

pub fn disk_status(s: DiskStatus, on: bool) -> String {
    let p = match s {
        DiskStatus::Normal => Paint::Green,
        DiskStatus::Warning => Paint::Yellow,
        DiskStatus::Failing | DiskStatus::Critical => Paint::Red,
    };
    paint(s.as_str(), p, on)
}

pub fn backup_status(s: BackupTaskStatus, on: bool) -> String {
    let p = match s {
        BackupTaskStatus::Idle => Paint::Dim,
        BackupTaskStatus::Running => Paint::Green,
        BackupTaskStatus::Suspended => Paint::Yellow,
        BackupTaskStatus::Error => Paint::Red,
    };
    paint(s.as_str(), p, on)
}

pub fn backup_result(r: BackupTaskResult, on: bool) -> String {
    let p = match r {
        BackupTaskResult::Success => Paint::Green,
        BackupTaskResult::Warning | BackupTaskResult::Skipped => Paint::Yellow,
        BackupTaskResult::Failed => Paint::Red,
        BackupTaskResult::Unknown => Paint::Dim,
    };
    paint(r.as_str(), p, on)
}

pub fn container_status(s: ContainerStatus, on: bool) -> String {
    let p = match s {
        ContainerStatus::Running => Paint::Green,
        ContainerStatus::Stopped => Paint::Dim,
        ContainerStatus::Paused | ContainerStatus::Restarting => Paint::Yellow,
        ContainerStatus::Dead => Paint::Red,
    };
    paint(s.as_str(), p, on)
}

pub fn update_status(s: UpdateStatus, on: bool) -> String {
    let p = match s {
        UpdateStatus::UpToDate => Paint::Green,
        UpdateStatus::UpdateAvailable => Paint::Yellow,
        UpdateStatus::Stale | UpdateStatus::Unknown => Paint::Dim,
    };
    paint(s.as_str(), p, on)
}

pub fn verdict(v: Verdict, on: bool) -> String {
    let p = match v {
        Verdict::Ok => Paint::Green,
        Verdict::Info => Paint::Blue,
        Verdict::Warn => Paint::Yellow,
        Verdict::Error => Paint::BoldRed,
        Verdict::Unknown => Paint::Dim,
    };
    paint(v.as_str(), p, on)
}

/// Volume use as `71.5%`, graded by the `status` thresholds; `-` when unknown.
pub fn usage(pct: Option<f64>, on: bool) -> String {
    pct.map_or_else(
        || "-".to_string(),
        |p| {
            let paint_with = if p >= VOLUME_ERROR_PCT {
                Paint::Red
            } else if p >= VOLUME_WARN_PCT {
                Paint::Yellow
            } else {
                Paint::Green
            };
            paint(&super::format::format_percent(p), paint_with, on)
        },
    )
}

/// `41°C`, green below the `status` warning level, yellow from it.
pub fn temperature(c: i64, on: bool) -> String {
    let p = if c >= DISK_WARN_C {
        Paint::Yellow
    } else {
        Paint::Green
    };
    paint(&format!("{c}°C"), p, on)
}

/// `config check` marks: ✓ ! ✗ –.
pub fn check_mark(s: Status, on: bool) -> String {
    let (mark, p) = match s {
        Status::Pass => ("✓", Paint::Green),
        Status::Warn => ("!", Paint::Yellow),
        Status::Fail => ("✗", Paint::Red),
        Status::Skip => ("–", Paint::Dim),
    };
    paint(mark, p, on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_wraps_only_when_on() {
        assert_eq!(paint("up", Paint::Green, true), "\x1b[32mup\x1b[0m");
        assert_eq!(paint("up", Paint::Green, false), "up");
    }

    #[test]
    fn usage_and_temperature_are_graded() {
        assert_eq!(usage(Some(71.52), true), "\x1b[32m71.5%\x1b[0m");
        assert_eq!(usage(Some(80.0), true), "\x1b[33m80.0%\x1b[0m");
        assert_eq!(usage(Some(95.0), true), "\x1b[31m95.0%\x1b[0m");
        assert_eq!(usage(None, true), "-");
        assert_eq!(temperature(49, true), "\x1b[32m49°C\x1b[0m");
        assert_eq!(temperature(50, true), "\x1b[33m50°C\x1b[0m");
        assert_eq!(temperature(50, false), "50°C");
    }

    #[test]
    fn verdicts_map_to_colours() {
        assert_eq!(verdict(Verdict::Ok, true), "\x1b[32mok\x1b[0m");
        assert_eq!(verdict(Verdict::Info, true), "\x1b[34minfo\x1b[0m");
        assert_eq!(verdict(Verdict::Warn, true), "\x1b[33mwarn\x1b[0m");
        assert_eq!(verdict(Verdict::Error, true), "\x1b[1;31merror\x1b[0m");
        assert_eq!(verdict(Verdict::Unknown, true), "\x1b[2munknown\x1b[0m");
    }

    #[test]
    fn statuses_map_to_colours() {
        assert_eq!(
            volume_status(VolumeStatus::Crashed, true),
            "\x1b[31mcrashed\x1b[0m"
        );
        assert_eq!(
            disk_status(DiskStatus::Warning, true),
            "\x1b[33mwarning\x1b[0m"
        );
        assert_eq!(
            backup_status(BackupTaskStatus::Idle, true),
            "\x1b[2midle\x1b[0m"
        );
        assert_eq!(
            backup_result(BackupTaskResult::Failed, true),
            "\x1b[31mfailed\x1b[0m"
        );
        assert_eq!(
            container_status(ContainerStatus::Running, true),
            "\x1b[32mrunning\x1b[0m"
        );
        assert_eq!(
            container_status(ContainerStatus::Stopped, true),
            "\x1b[2mstopped\x1b[0m"
        );
        assert_eq!(
            update_status(UpdateStatus::UpdateAvailable, true),
            "\x1b[33mupdateAvailable\x1b[0m"
        );
    }
}
