//! Container Manager output types: the spec's docker schemas and the
//! container update schemas, without `device` and `uri`.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::Serialize;

use super::{Ids, labels};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ContainerStatus {
    Running,
    Stopped,
    Paused,
    Restarting,
    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RestartPolicy {
    Always,
    UnlessStopped,
    OnFailure,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MountMode {
    Ro,
    Rw,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerResources {
    pub cpu_percent: f64,
    pub memory_bytes: i64,
    pub memory_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    /// The container's name.
    pub id: String,
    pub name: String,
    pub status: ContainerStatus,
    pub image: String,
    pub restart_count: i64,
    pub resources: ContainerResources,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContainerList {
    pub items: Vec<Container>,
}

impl Ids for ContainerList {
    fn ids(&self) -> Vec<String> {
        self.items.iter().map(|c| c.id.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDetail {
    #[serde(flatten)]
    pub container: Container,
    pub started_at: Timestamp,
    /// Only for a container that is not running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    pub exit_code: i64,
    pub oom_killed: bool,
    pub restart_policy: RestartPolicy,
    pub privileged: bool,
    /// Bytes; 0 means unlimited.
    pub memory_limit: i64,
    pub port_bindings: Vec<PortBinding>,
    pub networks: Vec<ContainerNetwork>,
    pub volume_bindings: Vec<VolumeMount>,
    pub env_variables: Vec<EnvVariable>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub labels: BTreeMap<String, String>,
}

impl std::ops::Deref for ContainerDetail {
    type Target = Container;
    fn deref(&self) -> &Container {
        &self.container
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortBinding {
    pub container_port: i64,
    pub host_port: i64,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainerNetwork {
    pub name: String,
    pub driver: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VolumeMount {
    pub source: String,
    pub destination: String,
    pub mode: MountMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvVariable {
    pub key: String,
    pub value: String,
}

/// `docker containers start|stop|restart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

impl Action {
    /// `Started`, `Stopped`, `Restarted`: hlctl's confirmation wording.
    pub fn past(self) -> &'static str {
        match self {
            Self::Start => "Started",
            Self::Stop => "Stopped",
            Self::Restart => "Restarted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DockerImage {
    /// The first 12 hex characters of the image's SHA-256 digest.
    pub id: String,
    pub repository: String,
    pub tags: Vec<String>,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DockerImageList {
    pub items: Vec<DockerImage>,
}

impl Ids for DockerImageList {
    fn ids(&self) -> Vec<String> {
        self.items.iter().map(|i| i.id.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerImageDetail {
    #[serde(flatten)]
    pub image: DockerImage,
    pub virtual_size: i64,
    pub created: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl std::ops::Deref for DockerImageDetail {
    type Target = DockerImage;
    fn deref(&self) -> &DockerImage {
        &self.image
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetwork {
    /// The network's name.
    pub id: String,
    pub name: String,
    pub connected_containers: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DockerNetworkList {
    pub items: Vec<DockerNetwork>,
}

impl Ids for DockerNetworkList {
    fn ids(&self) -> Vec<String> {
        self.items.iter().map(|n| n.id.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerNetworkDetail {
    #[serde(flatten)]
    pub network: DockerNetwork,
    pub driver: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subnet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_range: Option<String>,
    pub containers: Vec<String>,
}

impl std::ops::Deref for DockerNetworkDetail {
    type Target = DockerNetwork;
    fn deref(&self) -> &DockerNetwork {
        &self.network
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
    /// A release source is configured, but no release is known yet.
    Stale,
    /// No release source is configured for the image.
    Unknown,
}

/// A container whose image tag is tracked against upstream releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    /// The container's name.
    pub id: String,
    pub name: String,
    /// Always `container`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub status: UpdateStatus,
    pub current_version: String,
    pub latest_version: String,
    pub checked_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateList {
    pub items: Vec<Update>,
}

impl Ids for UpdateList {
    fn ids(&self) -> Vec<String> {
        self.items.iter().map(|u| u.id.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDetail {
    #[serde(flatten)]
    pub update: Update,
    /// The image without its tag.
    pub image: String,
    pub source: String,
    pub release_url: String,
    pub published_at: Timestamp,
}

impl std::ops::Deref for UpdateDetail {
    type Target = Update;
    fn deref(&self) -> &Update {
        &self.update
    }
}

labels!(ContainerStatus { Running => "running", Stopped => "stopped", Paused => "paused", Restarting => "restarting", Dead => "dead" });
labels!(RestartPolicy { Always => "always", UnlessStopped => "unlessStopped", OnFailure => "onFailure", No => "no" });
labels!(Protocol { Tcp => "tcp", Udp => "udp" });
labels!(MountMode { Ro => "ro", Rw => "rw" });
labels!(UpdateStatus { UpToDate => "upToDate", UpdateAvailable => "updateAvailable", Stale => "stale", Unknown => "unknown" });
