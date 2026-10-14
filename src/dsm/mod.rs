//! The Synology DSM Web API: the calls synoctl makes, their raw payloads,
//! and the HTTP client behind them.

pub mod types;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;

use std::thread::ScopedJoinHandle;

use crate::error::Result;
use types::{
    BackupTarget, BackupTaskDetail, BackupTaskList, BackupTaskStatus, ContainerDetailResponse,
    ContainerList, ContainerResources, ImageList, NetworkList, Storage, SystemInfo, Utilization,
};

/// Runs independent DSM reads concurrently, so a command waits for the
/// slowest call rather than the sum of all of them, and binds each result
/// to its name:
///
/// ```ignore
/// fetch!(list = self.api.containers(), res = self.api.container_resources());
/// ```
///
/// Returns the first error in argument order from the enclosing function.
macro_rules! fetch {
    ($($name:ident = $read:expr),+ $(,)?) => {
        let ($($name,)+) = std::thread::scope(|s| {
            $(let $name = s.spawn(|| $read);)+
            Ok::<_, $crate::error::Error>(($($crate::dsm::joined($name)?,)+))
        })?;
    };
}
pub(crate) use fetch;

/// Result of a `fetch!` thread; a panic in the read is re-raised here.
pub(crate) fn joined<T>(handle: ScopedJoinHandle<'_, Result<T>>) -> Result<T> {
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// The DSM calls synoctl makes. Implemented by the HTTP client and by
/// `FixtureApi`. `Sync` so independent reads can run on parallel
/// threads.
pub trait DsmApi: Sync {
    /// The NAS address, for messages.
    fn host(&self) -> &str;
    fn system_info(&self) -> Result<SystemInfo>;
    fn system_utilization(&self) -> Result<Utilization>;
    fn storage(&self) -> Result<Storage>;
    fn containers(&self) -> Result<ContainerList>;
    /// `Error::NotFound` when no container has this name.
    fn container(&self, name: &str) -> Result<ContainerDetailResponse>;
    fn container_resources(&self) -> Result<ContainerResources>;
    fn start_container(&self, name: &str) -> Result<()>;
    fn stop_container(&self, name: &str) -> Result<()>;
    fn restart_container(&self, name: &str) -> Result<()>;
    fn images(&self) -> Result<ImageList>;
    fn networks(&self) -> Result<NetworkList>;
    fn backup_tasks(&self) -> Result<BackupTaskList>;
    fn backup_task(&self, task_id: i64) -> Result<BackupTaskDetail>;
    fn backup_status(&self, task_id: i64) -> Result<BackupTaskStatus>;
    fn backup_target(&self, task_id: i64) -> Result<BackupTarget>;
}

/// "Container Manager is not installed on <host>".
pub fn docker_missing(host: &str) -> crate::error::Error {
    crate::error::Error::NotInstalled(format!("Container Manager is not installed on {host}"))
}

/// "Hyper Backup is not installed on <host>".
pub fn backup_missing(host: &str) -> crate::error::Error {
    crate::error::Error::NotInstalled(format!("Hyper Backup is not installed on {host}"))
}

/// "Container not found: <name>".
pub fn container_not_found(name: &str) -> crate::error::Error {
    crate::error::Error::NotFound(format!("Container not found: {name}"))
}
pub mod client;
pub mod session;
