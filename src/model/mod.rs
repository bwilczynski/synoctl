//! Output types: the homelab-api-spec docker, storage and system schemas,
//! without `device` and `uri` (one NAS per config). Field names follow the
//! spec; `-o json` prints these.

use jiff::Timestamp;
use serde::Serialize;

pub mod docker;

/// `as_str()` for an enum: the same text its JSON value has, for tables.
macro_rules! labels {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }
    };
}
pub(crate) use labels;

/// Lists that `-o ids` can print, one ID per line.
pub trait Ids {
    fn ids(&self) -> Vec<String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub model: String,
    pub firmware: String,
    pub ram_mb: i64,
    pub uptime_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemUtilization {
    pub sampled_at: Timestamp,
    pub cpu: CpuUsage,
    pub memory: MemoryUsage,
    pub network: Vec<NetworkInterfaceUsage>,
    pub disks: Vec<DiskIo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuUsage {
    pub user_percent: i64,
    pub system_percent: i64,
    pub total_percent: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryUsage {
    pub total_bytes: i64,
    pub available_bytes: i64,
    pub used_percent: i64,
    pub swap_total_bytes: i64,
    pub swap_used_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterfaceUsage {
    pub name: String,
    pub rx_bytes_per_sec: i64,
    pub tx_bytes_per_sec: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskIo {
    pub name: String,
    pub read_ops_per_sec: i64,
    pub write_ops_per_sec: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VolumeStatus {
    Normal,
    Degraded,
    Repairing,
    Crashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiskStatus {
    Normal,
    Warning,
    Failing,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub id: String,
    pub name: String,
    pub status: VolumeStatus,
    pub raid_type: String,
    pub file_system: String,
    pub total_bytes: i64,
    pub used_bytes: i64,
}

impl Volume {
    /// Share of the volume in use; `None` when its size is unknown.
    pub fn used_percent(&self) -> Option<f64> {
        (self.total_bytes > 0).then(|| self.used_bytes as f64 * 100.0 / self.total_bytes as f64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VolumeList {
    pub items: Vec<Volume>,
}

impl Ids for VolumeList {
    fn ids(&self) -> Vec<String> {
        self.items.iter().map(|v| v.id.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeDetail {
    #[serde(flatten)]
    pub volume: Volume,
    pub mount_path: String,
    pub pool_status: VolumeStatus,
    pub disks: Vec<VolumeDisk>,
}

impl std::ops::Deref for VolumeDetail {
    type Target = Volume;
    fn deref(&self) -> &Volume {
        &self.volume
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeDisk {
    pub id: String,
    pub model: String,
    pub status: DiskStatus,
    pub total_bytes: i64,
    pub temperature_celsius: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupTaskStatus {
    Idle,
    Running,
    /// Hyper Backup's integrity check ("Checking backup integrity...").
    Checking,
    Suspended,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupTaskResult {
    Success,
    /// DSM's "Partial success".
    Warning,
    Failed,
    Canceled,
    /// No backup yet, waiting to check the server, or not readable.
    Unknown,
}

/// A Hyper Backup task and the outcome of its last run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupTask {
    /// DSM's numeric task ID.
    pub id: String,
    pub name: String,
    /// `hyperBackup`, `glacierBackup`, or DSM's own type.
    #[serde(rename = "type")]
    pub kind: String,
    pub status: BackupTaskStatus,
    pub last_result: BackupTaskResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupTaskList {
    pub items: Vec<BackupTask>,
}

impl Ids for BackupTaskList {
    fn ids(&self) -> Vec<String> {
        self.items.iter().map(|t| t.id.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupTaskDetail {
    #[serde(flatten)]
    pub task: BackupTask,
    /// The last successful run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<Timestamp>,
    /// Bytes stored at the target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folders: Option<Vec<String>>,
}

impl std::ops::Deref for BackupTaskDetail {
    type Target = BackupTask;
    fn deref(&self) -> &BackupTask {
        &self.task
    }
}

/// How an area of `status` looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Info,
    Warn,
    Error,
    /// Its data could not be read.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusArea {
    System,
    Volumes,
    Disks,
    Containers,
    Backups,
    Updates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusRow {
    pub area: StatusArea,
    pub verdict: Verdict,
    pub summary: String,
    /// What is wrong, one line each.
    pub problems: Vec<String>,
}

/// `status`: one row per area, in display order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NasStatus {
    pub rows: Vec<StatusRow>,
}

impl NasStatus {
    pub fn has_error(&self) -> bool {
        self.rows.iter().any(|r| r.verdict == Verdict::Error)
    }
}

labels!(VolumeStatus { Normal => "normal", Degraded => "degraded", Repairing => "repairing", Crashed => "crashed" });
labels!(DiskStatus { Normal => "normal", Warning => "warning", Failing => "failing", Critical => "critical" });
labels!(BackupTaskStatus { Idle => "idle", Running => "running", Checking => "checking", Suspended => "suspended", Error => "error" });
labels!(BackupTaskResult { Success => "success", Warning => "warning", Failed => "failed", Canceled => "canceled", Unknown => "unknown" });
labels!(Verdict { Ok => "ok", Info => "info", Warn => "warn", Error => "error", Unknown => "unknown" });

#[cfg(test)]
mod tests {
    use super::docker::*;
    use super::*;

    fn json(v: impl Serialize) -> String {
        serde_json::to_value(v)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn labels_match_json() {
        for v in [
            VolumeStatus::Normal,
            VolumeStatus::Degraded,
            VolumeStatus::Repairing,
            VolumeStatus::Crashed,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        for v in [
            DiskStatus::Normal,
            DiskStatus::Warning,
            DiskStatus::Failing,
            DiskStatus::Critical,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        for v in [
            BackupTaskStatus::Idle,
            BackupTaskStatus::Running,
            BackupTaskStatus::Checking,
            BackupTaskStatus::Suspended,
            BackupTaskStatus::Error,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        for v in [
            BackupTaskResult::Success,
            BackupTaskResult::Warning,
            BackupTaskResult::Failed,
            BackupTaskResult::Canceled,
            BackupTaskResult::Unknown,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        for v in [
            Verdict::Ok,
            Verdict::Info,
            Verdict::Warn,
            Verdict::Error,
            Verdict::Unknown,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        for v in [
            ContainerStatus::Running,
            ContainerStatus::Stopped,
            ContainerStatus::Paused,
            ContainerStatus::Restarting,
            ContainerStatus::Dead,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        for v in [
            RestartPolicy::Always,
            RestartPolicy::UnlessStopped,
            RestartPolicy::OnFailure,
            RestartPolicy::No,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        for v in [
            UpdateStatus::UpToDate,
            UpdateStatus::UpdateAvailable,
            UpdateStatus::Stale,
            UpdateStatus::Unknown,
            UpdateStatus::Incomparable,
        ] {
            assert_eq!(v.as_str(), json(v));
        }
        assert_eq!(Protocol::Udp.as_str(), json(Protocol::Udp));
        assert_eq!(MountMode::Ro.as_str(), json(MountMode::Ro));
    }
}
