//! Raw DSM Web API payloads, ported from
//! homelab-api/internal/adapters/synology.go. Every struct defaults missing
//! fields, so one absent key never fails a whole command.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

/// `{"success": …, "data": …, "error": {"code": …}}`, every DSM response.
#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    #[serde(default)]
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ErrorBody {
    #[serde(default)]
    pub code: i64,
}

/// A JSON `null` read as the type's default (DSM sends `"tags": null`).
pub fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

// --- System ---

/// `SYNO.Core.System` `info`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SystemInfo {
    pub firmware_ver: String,
    pub model: String,
    /// MB.
    pub ram_size: i64,
    /// `H:M:S`, hours unbounded.
    pub up_time: String,
    /// °C.
    pub sys_temp: i64,
    pub sys_tempwarn: bool,
    pub temperature_warning: bool,
}

/// `SYNO.Core.System.Utilization` `get`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Utilization {
    pub cpu: CpuUsage,
    pub memory: MemoryUsage,
    #[serde(deserialize_with = "null_as_default")]
    pub network: Vec<NetworkStat>,
    pub disk: DiskStats,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CpuUsage {
    pub user_load: i64,
    pub system_load: i64,
    pub other_load: i64,
}

/// Every value in KB.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MemoryUsage {
    pub memory_size: i64,
    pub total_real: i64,
    pub avail_real: i64,
    pub real_usage: i64,
    pub total_swap: i64,
    pub avail_swap: i64,
}

/// Bytes per second; device `total` is the sum of the others.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NetworkStat {
    pub device: String,
    pub rx: i64,
    pub tx: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DiskStats {
    #[serde(deserialize_with = "null_as_default")]
    pub disk: Vec<DiskStat>,
}

/// Operations per second.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DiskStat {
    pub device: String,
    pub read_access: i64,
    pub write_access: i64,
}

// --- Storage ---

/// `SYNO.Storage.CGI.Storage` `load_info`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Storage {
    #[serde(deserialize_with = "null_as_default")]
    pub volumes: Vec<Volume>,
    #[serde(deserialize_with = "null_as_default")]
    pub disks: Vec<Disk>,
    #[serde(rename = "storagePools", deserialize_with = "null_as_default")]
    pub storage_pools: Vec<Pool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Volume {
    pub id: String,
    pub vol_path: String,
    pub status: String,
    pub fs_type: String,
    #[serde(rename = "raidType")]
    pub raid_type: String,
    pub pool_path: String,
    pub size: VolumeSize,
}

/// Bytes, as decimal strings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VolumeSize {
    pub total: String,
    pub used: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Disk {
    pub id: String,
    pub model: String,
    /// Bytes, as a decimal string.
    pub size_total: String,
    pub status: String,
    /// °C.
    pub temp: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Pool {
    pub id: String,
    #[serde(deserialize_with = "null_as_default")]
    pub disks: Vec<String>,
    #[serde(rename = "raidType")]
    pub raid_type: String,
    pub status: String,
}

// --- Docker ---

/// `SYNO.Docker.Container` `list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContainerList {
    #[serde(deserialize_with = "null_as_default")]
    pub containers: Vec<Container>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    #[serde(rename = "State")]
    pub state: ContainerState,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ContainerState {
    pub dead: bool,
    pub paused: bool,
    pub restarting: bool,
    pub running: bool,
    pub status: String,
    /// RFC 3339.
    pub started_at: String,
    /// RFC 3339; `0001-01-01T00:00:00Z` when it never finished.
    pub finished_at: String,
    pub exit_code: i64,
    #[serde(rename = "OOMKilled")]
    pub oom_killed: bool,
}

/// `SYNO.Docker.Container` `get`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContainerDetailResponse {
    pub details: ContainerDetail,
    pub profile: ContainerProfile,
}

/// Synology's container profile (what Container Manager edits).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContainerProfile {
    pub name: String,
    pub image: String,
    #[serde(deserialize_with = "null_as_default")]
    pub env_variables: Vec<EnvVariable>,
    #[serde(rename = "network", deserialize_with = "null_as_default")]
    pub networks: Vec<ProfileNetwork>,
    #[serde(deserialize_with = "null_as_default")]
    pub port_bindings: Vec<ProfilePortBinding>,
    #[serde(deserialize_with = "null_as_default")]
    pub volume_bindings: Vec<VolumeBinding>,
    pub privileged: bool,
    /// Bytes; 0 means unlimited.
    pub memory_limit: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EnvVariable {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProfileNetwork {
    pub name: String,
    pub driver: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProfilePortBinding {
    pub container_port: i64,
    pub host_port: i64,
    /// `tcp` or `udp`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// DSM uses `host_absolute_path` for host paths and `host_volume_file` for
/// DSM-managed folders.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VolumeBinding {
    pub host_absolute_path: String,
    pub host_volume_file: String,
    pub mount_point: String,
    /// `ro` or `rw`.
    #[serde(rename = "type")]
    pub kind: String,
}

impl VolumeBinding {
    /// The host side, preferring `host_absolute_path`.
    pub fn host_path(&self) -> &str {
        if self.host_absolute_path.is_empty() {
            &self.host_volume_file
        } else {
            &self.host_absolute_path
        }
    }
}

/// Docker's own inspect output.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ContainerDetail {
    pub name: String,
    pub restart_count: i64,
    pub state: ContainerState,
    pub config: ContainerConfig,
    pub host_config: HostConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ContainerConfig {
    pub image: String,
    #[serde(deserialize_with = "null_as_default")]
    pub cmd: Vec<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub entrypoint: Vec<String>,
    #[serde(deserialize_with = "null_as_default")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct HostConfig {
    pub restart_policy: RestartPolicy,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RestartPolicy {
    /// `always`, `no`, `unless-stopped`, `on-failure`.
    pub name: String,
}

/// `SYNO.Docker.Container.Resource` `get`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContainerResources {
    #[serde(deserialize_with = "null_as_default")]
    pub resources: Vec<ContainerResource>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ContainerResource {
    pub name: String,
    pub cpu: f64,
    /// Bytes.
    pub memory: i64,
    #[serde(rename = "memoryPercent")]
    pub memory_percent: f64,
}

/// `SYNO.Docker.Image` `list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImageList {
    #[serde(deserialize_with = "null_as_default")]
    pub images: Vec<Image>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Image {
    /// `sha256:<hex>`.
    pub id: String,
    pub repository: String,
    #[serde(deserialize_with = "null_as_default")]
    pub tags: Vec<String>,
    pub size: i64,
    pub virtual_size: i64,
    /// Unix seconds.
    pub created: i64,
    pub description: String,
}

/// `SYNO.Docker.Network` `list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NetworkList {
    #[serde(deserialize_with = "null_as_default")]
    pub network: Vec<Network>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub gateway: String,
    pub subnet: String,
    pub iprange: String,
    #[serde(deserialize_with = "null_as_default")]
    pub containers: Vec<String>,
}

// --- Hyper Backup ---

/// `SYNO.Backup.Task` `list`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackupTaskList {
    #[serde(deserialize_with = "null_as_default")]
    pub task_list: Vec<BackupTask>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackupTask {
    pub task_id: i64,
    pub name: String,
    /// `backupable`, `running`, `suspend`, `error`.
    pub state: String,
    pub status: String,
    /// e.g. `image:image_local`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// `SYNO.Backup.Task` `get` with `additional=["repository","schedule"]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackupTaskDetail {
    pub task_id: i64,
    pub name: String,
    pub source: BackupSource,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackupSource {
    #[serde(deserialize_with = "null_as_default")]
    pub folder_list: Vec<BackupFolder>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackupFolder {
    #[serde(rename = "fullPath")]
    pub full_path: String,
}

/// `SYNO.Backup.Task` `status`. Times are `YYYY/MM/DD HH:MM` in the NAS's
/// zone.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackupTaskStatus {
    pub task_id: i64,
    pub state: String,
    /// `done`, `error`, `skip`, …
    pub last_bkp_result: String,
    pub last_bkp_error_code: i64,
    pub last_bkp_success_time: String,
    pub last_bkp_time: String,
    pub last_bkp_end_time: String,
    pub next_bkp_time: String,
}

/// `SYNO.Backup.Target` `get`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackupTarget {
    /// Bytes.
    pub used_size: i64,
    pub is_online: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nulls_become_empty() {
        let img: Image = serde_json::from_str(r#"{"id":"sha256:ab","tags":null}"#).unwrap();
        assert!(img.tags.is_empty());
        let n: Network = serde_json::from_str(r#"{"name":"x","containers":null}"#).unwrap();
        assert!(n.containers.is_empty());
        let c: ContainerConfig = serde_json::from_str(r#"{"Labels":null,"Cmd":null}"#).unwrap();
        assert!(c.labels.is_empty() && c.cmd.is_empty());
    }

    #[test]
    fn host_path_prefers_absolute() {
        let b = |abs: &str, vol: &str| VolumeBinding {
            host_absolute_path: abs.into(),
            host_volume_file: vol.into(),
            ..Default::default()
        };
        assert_eq!(b("/etc/localtime", "").host_path(), "/etc/localtime");
        assert_eq!(b("", "/docker/x").host_path(), "/docker/x");
    }
}
