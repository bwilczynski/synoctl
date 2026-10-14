//! Fixture-backed `DsmApi` for unit tests and the hidden `--fixtures` flag.
//! Reads the `dsm-*.json` files in a directory (each a DSM envelope); the
//! Docker and Hyper Backup files are optional, like those packages.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;

use super::types::{
    BackupTarget, BackupTaskDetail, BackupTaskList, BackupTaskStatus, ContainerDetailResponse,
    ContainerList, ContainerResources, ImageList, NetworkList, Storage, SystemInfo, Utilization,
};
use super::{DsmApi, backup_missing, container_not_found, docker_missing};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct FixtureApi {
    pub system_info: SystemInfo,
    pub utilization: Utilization,
    pub storage: Storage,
    /// `None` when `dsm-containers.json` is absent: Container Manager is not
    /// installed, and every Docker call says so.
    pub containers: Option<ContainerList>,
    /// `dsm-container-<name>.json`, by name.
    pub details: BTreeMap<String, ContainerDetailResponse>,
    pub resources: ContainerResources,
    pub images: ImageList,
    pub networks: NetworkList,
    /// `None` when `dsm-backup-tasks.json` is absent: Hyper Backup is not
    /// installed.
    pub backup_tasks: Option<BackupTaskList>,
    /// `dsm-backup-task-<id>.json`, `-status-`, `-target-`, by task ID; a
    /// missing one fails that lookup.
    pub backup_details: BTreeMap<i64, BackupTaskDetail>,
    pub backup_statuses: BTreeMap<i64, BackupTaskStatus>,
    pub backup_targets: BTreeMap<i64, BackupTarget>,
    /// Reads that fail, by method name (`"storage"`, …).
    pub fail_only: Vec<&'static str>,
    /// When true every call fails, simulating an unreachable NAS.
    pub fail: bool,
    /// Container actions performed, e.g. `"restart prometheus"`; shared by
    /// clones, so a test can watch a `FixtureApi` it handed to a `Service`.
    pub actions: Arc<Mutex<Vec<String>>>,
}

const HOST: &str = "fixture";

impl FixtureApi {
    /// `tests/fixtures` inside this crate.
    pub fn default_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let mut details = BTreeMap::new();
        let (mut backup_details, mut backup_statuses, mut backup_targets) =
            (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        let entries = std::fs::read_dir(dir)
            .map_err(|e| Error::Other(format!("fixtures {}: {e}", dir.display())))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            if let Some(container) = stem.strip_prefix("dsm-container-") {
                if container != "resources" {
                    details.insert(container.to_string(), read(dir, &name)?);
                }
            } else if let Some(id) = task_id(stem, "dsm-backup-task-") {
                backup_details.insert(id, read(dir, &name)?);
            } else if let Some(id) = task_id(stem, "dsm-backup-status-") {
                backup_statuses.insert(id, read(dir, &name)?);
            } else if let Some(id) = task_id(stem, "dsm-backup-target-") {
                backup_targets.insert(id, read(dir, &name)?);
            }
        }
        Ok(Self {
            system_info: read(dir, "dsm-system-info.json")?,
            utilization: read(dir, "dsm-system-utilization.json")?,
            storage: read(dir, "dsm-storage.json")?,
            containers: read_if_present(dir, "dsm-containers.json")?,
            details,
            resources: read_if_present(dir, "dsm-container-resources.json")?.unwrap_or_default(),
            images: read_if_present(dir, "dsm-images.json")?.unwrap_or_default(),
            networks: read_if_present(dir, "dsm-networks.json")?.unwrap_or_default(),
            backup_tasks: read_if_present(dir, "dsm-backup-tasks.json")?,
            backup_details,
            backup_statuses,
            backup_targets,
            ..Default::default()
        })
    }

    /// Fails when every read fails (`fail`) or this one does (`fail_only`).
    fn check(&self, read: &'static str) -> Result<()> {
        if self.fail || self.fail_only.contains(&read) {
            return Err(Error::Dsm("synology request: fixture failure".into()));
        }
        Ok(())
    }

    fn docker(&self, read: &'static str) -> Result<&ContainerList> {
        self.check(read)?;
        self.containers.as_ref().ok_or_else(|| docker_missing(HOST))
    }

    fn backup(&self, read: &'static str) -> Result<&BackupTaskList> {
        self.check(read)?;
        self.backup_tasks
            .as_ref()
            .ok_or_else(|| backup_missing(HOST))
    }

    fn act(&self, verb: &str, name: &str) -> Result<()> {
        let list = self.docker("container_action")?;
        if !list.containers.iter().any(|c| c.name == name) {
            return Err(container_not_found(name));
        }
        self.actions
            .lock()
            .expect("actions lock")
            .push(format!("{verb} {name}"));
        Ok(())
    }
}

fn task_id(stem: &str, prefix: &str) -> Option<i64> {
    stem.strip_prefix(prefix)?.parse().ok()
}

fn read<T: DeserializeOwned>(dir: &Path, name: &str) -> Result<T> {
    let path = dir.join(name);
    let fail = |e: String| Error::Other(format!("fixture {}: {e}", path.display()));
    let text = std::fs::read_to_string(&path).map_err(|e| fail(e.to_string()))?;
    #[derive(serde::Deserialize)]
    struct Data<T> {
        data: T,
    }
    serde_json::from_str::<Data<T>>(&text)
        .map(|d| d.data)
        .map_err(|e| fail(e.to_string()))
}

fn read_if_present<T: DeserializeOwned>(dir: &Path, name: &str) -> Result<Option<T>> {
    dir.join(name).exists().then(|| read(dir, name)).transpose()
}

fn missing(what: &str, id: i64) -> Error {
    Error::Dsm(format!("fixture: no {what} for task {id}"))
}

impl DsmApi for FixtureApi {
    fn host(&self) -> &str {
        HOST
    }

    fn system_info(&self) -> Result<SystemInfo> {
        self.check("system_info")?;
        Ok(self.system_info.clone())
    }

    fn system_utilization(&self) -> Result<Utilization> {
        self.check("system_utilization")?;
        Ok(self.utilization.clone())
    }

    fn storage(&self) -> Result<Storage> {
        self.check("storage")?;
        Ok(self.storage.clone())
    }

    fn containers(&self) -> Result<ContainerList> {
        self.docker("containers").cloned()
    }

    fn container(&self, name: &str) -> Result<ContainerDetailResponse> {
        self.docker("container")?;
        self.details
            .get(name)
            .cloned()
            .ok_or_else(|| container_not_found(name))
    }

    fn container_resources(&self) -> Result<ContainerResources> {
        self.docker("container_resources")?;
        Ok(self.resources.clone())
    }

    fn start_container(&self, name: &str) -> Result<()> {
        self.act("start", name)
    }

    fn stop_container(&self, name: &str) -> Result<()> {
        self.act("stop", name)
    }

    fn restart_container(&self, name: &str) -> Result<()> {
        self.act("restart", name)
    }

    fn images(&self) -> Result<ImageList> {
        self.docker("images")?;
        Ok(self.images.clone())
    }

    fn networks(&self) -> Result<NetworkList> {
        self.docker("networks")?;
        Ok(self.networks.clone())
    }

    fn backup_tasks(&self) -> Result<BackupTaskList> {
        self.backup("backup_tasks").cloned()
    }

    fn backup_task(&self, task_id: i64) -> Result<BackupTaskDetail> {
        self.backup("backup_task")?;
        let detail = self.backup_details.get(&task_id);
        detail.cloned().ok_or_else(|| missing("detail", task_id))
    }

    fn backup_status(&self, task_id: i64) -> Result<BackupTaskStatus> {
        self.backup("backup_status")?;
        let status = self.backup_statuses.get(&task_id);
        status.cloned().ok_or_else(|| missing("status", task_id))
    }

    fn backup_target(&self, task_id: i64) -> Result<BackupTarget> {
        self.backup("backup_target")?;
        let target = self.backup_targets.get(&task_id);
        target.cloned().ok_or_else(|| missing("target", task_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api() -> FixtureApi {
        FixtureApi::load(&FixtureApi::default_dir()).unwrap()
    }

    #[test]
    fn loads_every_fixture() {
        let api = api();
        let info = api.system_info().unwrap();
        assert_eq!(info.model, "DS918+");
        assert_eq!(info.up_time, "100:00:00");
        assert_eq!(info.sys_temp, 43);
        assert_eq!(
            api.system_utilization().unwrap().memory.total_real,
            16234636
        );
        let s = api.storage().unwrap();
        assert_eq!(
            (s.volumes.len(), s.disks.len(), s.storage_pools.len()),
            (1, 4, 1)
        );
        assert_eq!(s.volumes[0].size.total, "11508017246208");
        let c = api.containers().unwrap();
        assert_eq!(
            c.containers
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["immich_server", "immich_machine_learning", "prometheus"]
        );
        assert!(!c.containers[2].state.running);
        assert_eq!(
            api.container("immich_server")
                .unwrap()
                .profile
                .port_bindings[0]
                .host_port,
            12080
        );
        assert_eq!(api.container_resources().unwrap().resources.len(), 3);
        assert_eq!(api.images().unwrap().images.len(), 4);
        assert_eq!(api.networks().unwrap().network[3].name, "host");
        assert_eq!(api.backup_tasks().unwrap().task_list.len(), 2);
        let status = api.backup_status(3).unwrap();
        assert_eq!(status.last_bkp_error_code, 4401);
        assert_eq!(status.last_bkp_end_time, "2026/04/24 02:31");
        assert_eq!(api.backup_target(3).unwrap().used_size, 3206674163);
        assert_eq!(
            api.backup_task(3).unwrap().source.folder_list[1].full_path,
            "/volume1/Documents"
        );
    }

    #[test]
    fn unknown_container_is_not_found() {
        assert_eq!(
            api().container("nope").unwrap_err().to_string(),
            "Container not found: nope"
        );
    }

    #[test]
    fn missing_docker_fixtures_mean_not_installed() {
        let api = FixtureApi {
            containers: None,
            ..api()
        };
        let e = api.images().unwrap_err();
        assert!(matches!(e, Error::NotInstalled(_)));
        assert_eq!(
            e.to_string(),
            "Container Manager is not installed on fixture"
        );
    }

    #[test]
    fn missing_backup_fixtures_mean_not_installed() {
        let api = FixtureApi {
            backup_tasks: None,
            ..api()
        };
        assert_eq!(
            api.backup_tasks().unwrap_err().to_string(),
            "Hyper Backup is not installed on fixture"
        );
    }

    #[test]
    fn task_without_status_fixture_fails_only_that_lookup() {
        let api = api();
        assert!(api.backup_status(4).is_err());
        assert!(api.backup_status(3).is_ok());
    }

    #[test]
    fn load_skips_absent_optional_files() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "dsm-system-info.json",
            "dsm-system-utilization.json",
            "dsm-storage.json",
        ] {
            std::fs::copy(FixtureApi::default_dir().join(f), dir.path().join(f)).unwrap();
        }
        let api = FixtureApi::load(dir.path()).unwrap();
        assert!(matches!(api.containers(), Err(Error::NotInstalled(_))));
        assert!(matches!(api.backup_tasks(), Err(Error::NotInstalled(_))));
    }

    #[test]
    fn records_container_actions() {
        let api = api();
        api.restart_container("prometheus").unwrap();
        api.stop_container("prometheus").unwrap();
        assert_eq!(
            *api.actions.lock().unwrap(),
            ["restart prometheus", "stop prometheus"]
        );
        assert!(api.start_container("nope").is_err());
    }

    #[test]
    fn fail_flags_fail_reads() {
        let api = FixtureApi {
            fail_only: vec!["storage"],
            ..api()
        };
        assert!(api.storage().is_err());
        assert!(api.system_info().is_ok());
        let api = FixtureApi { fail: true, ..api };
        assert!(api.system_info().is_err());
    }
}
