//! `docker containers`, from homelab-api/internal/docker/containers_service.go.

use jiff::Timestamp;

use crate::dsm::fetch;
use crate::dsm::types::{self, ContainerResource, ContainerState};
use crate::error::Result;
use crate::model::docker::{
    Action, Container, ContainerDetail, ContainerList, ContainerNetwork, ContainerResources,
    ContainerStatus, EnvVariable, MountMode, PortBinding, Protocol, RestartPolicy, VolumeMount,
};
use crate::service::Service;

impl Service {
    /// Every container with its CPU and memory use; `status` keeps only
    /// containers in that state.
    pub fn list_containers(&self, status: Option<ContainerStatus>) -> Result<ContainerList> {
        fetch!(
            list = self.api.containers(),
            res = self.api.container_resources(),
        );
        let items = list
            .containers
            .iter()
            .map(|c| Container {
                id: c.name.clone(),
                name: c.name.clone(),
                status: container_status(&c.state),
                image: c.image.clone(),
                resources: resources(find(&res.resources, &c.name)),
            })
            .filter(|c| status.is_none_or(|s| c.status == s))
            .collect();
        Ok(ContainerList { items })
    }

    pub fn get_container(&self, name: &str) -> Result<ContainerDetail> {
        fetch!(
            d = self.api.container(name),
            res = self.api.container_resources(),
        );
        let state = &d.details.state;
        let running = state.running;
        Ok(ContainerDetail {
            container: Container {
                id: d.profile.name.clone(),
                name: d.profile.name.clone(),
                status: container_status(state),
                image: d.profile.image.clone(),
                resources: resources(find(&res.resources, name)),
            },
            restart_count: d.details.restart_count,
            started_at: state.started_at.parse().unwrap_or(Timestamp::UNIX_EPOCH),
            finished_at: (!running)
                .then(|| state.finished_at.parse::<Timestamp>().ok())
                .flatten()
                .filter(|t| t.as_second() > 0),
            exit_code: state.exit_code,
            oom_killed: state.oom_killed,
            restart_policy: restart_policy(&d.details.host_config.restart_policy.name),
            privileged: d.profile.privileged,
            memory_limit: d.profile.memory_limit,
            port_bindings: d.profile.port_bindings.iter().map(port_binding).collect(),
            networks: d
                .profile
                .networks
                .iter()
                .map(|n| ContainerNetwork {
                    name: n.name.clone(),
                    driver: n.driver.clone(),
                })
                .collect(),
            volume_bindings: d.profile.volume_bindings.iter().map(volume_mount).collect(),
            env_variables: d
                .profile
                .env_variables
                .iter()
                .map(|e| EnvVariable {
                    key: e.key.clone(),
                    value: e.value.clone(),
                })
                .collect(),
            entrypoint: d.details.config.entrypoint.clone(),
            cmd: d.details.config.cmd.clone(),
            labels: d.details.config.labels.clone(),
        })
    }

    pub fn container_action(&self, name: &str, action: Action) -> Result<()> {
        match action {
            Action::Start => self.api.start_container(name),
            Action::Stop => self.api.stop_container(name),
            Action::Restart => self.api.restart_container(name),
        }
    }
}

fn find<'a>(res: &'a [ContainerResource], name: &str) -> Option<&'a ContainerResource> {
    res.iter().find(|r| r.name == name)
}

fn resources(r: Option<&ContainerResource>) -> ContainerResources {
    ContainerResources {
        cpu_percent: r.map_or(0.0, |r| r.cpu),
        memory_bytes: r.map_or(0, |r| r.memory),
        memory_percent: r.map_or(0.0, |r| r.memory_percent),
    }
}

fn port_binding(p: &types::ProfilePortBinding) -> PortBinding {
    PortBinding {
        container_port: p.container_port,
        host_port: p.host_port,
        protocol: if p.kind == "udp" {
            Protocol::Udp
        } else {
            Protocol::Tcp
        },
    }
}

fn volume_mount(v: &types::VolumeBinding) -> VolumeMount {
    VolumeMount {
        source: v.host_path().to_string(),
        destination: v.mount_point.clone(),
        mode: if v.kind == "ro" {
            MountMode::Ro
        } else {
            MountMode::Rw
        },
    }
}

/// Dead, restarting, paused, running, otherwise stopped.
pub(crate) fn container_status(s: &ContainerState) -> ContainerStatus {
    if s.dead {
        ContainerStatus::Dead
    } else if s.restarting {
        ContainerStatus::Restarting
    } else if s.paused {
        ContainerStatus::Paused
    } else if s.running {
        ContainerStatus::Running
    } else {
        ContainerStatus::Stopped
    }
}

pub(crate) fn restart_policy(name: &str) -> RestartPolicy {
    match name {
        "always" => RestartPolicy::Always,
        "unless-stopped" => RestartPolicy::UnlessStopped,
        "on-failure" => RestartPolicy::OnFailure,
        _ => RestartPolicy::No,
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::dsm::types::ContainerState;
    use crate::model::Ids;
    use crate::model::docker::{
        Action, ContainerResources, MountMode, PortBinding, Protocol, RestartPolicy, VolumeMount,
    };
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_joins_resources_by_name() {
        let l = svc(fixtures()).list_containers(None).unwrap();
        assert_eq!(
            l.items
                .iter()
                .map(|c| (c.id.as_str(), c.status))
                .collect::<Vec<_>>(),
            [
                ("immich_server", ContainerStatus::Running),
                ("immich_machine_learning", ContainerStatus::Running),
                ("prometheus", ContainerStatus::Stopped)
            ]
        );
        assert_eq!(l.items[0].name, "immich_server");
        assert_eq!(l.items[0].image, "ghcr.io/immich-app/immich-server:v2.6.3");
        assert_eq!(
            l.items[0].resources,
            ContainerResources {
                cpu_percent: 0.325,
                memory_bytes: 394334208,
                memory_percent: 2.372
            }
        );
        let item = serde_json::to_value(&l.items[0]).unwrap();
        assert!(item.get("restartCount").is_none(), "{item}");
    }

    #[test]
    fn get_reports_the_restart_count() {
        let mut api = fixtures();
        api.details
            .get_mut("immich_server")
            .unwrap()
            .details
            .restart_count = 7;
        let d = svc(api).get_container("immich_server").unwrap();
        assert_eq!(serde_json::to_value(&d).unwrap()["restartCount"], 7);
    }

    #[test]
    fn container_without_resources_has_zeros() {
        let mut api = fixtures();
        api.resources.resources.clear();
        let l = svc(api).list_containers(None).unwrap();
        assert_eq!(l.items[2].resources.memory_bytes, 0);
    }

    #[test]
    fn list_filters_by_status() {
        let l = svc(fixtures())
            .list_containers(Some(ContainerStatus::Stopped))
            .unwrap();
        assert_eq!(l.ids(), ["prometheus"]);
    }

    #[test]
    fn get_maps_profile_and_details() {
        let d = svc(fixtures()).get_container("immich_server").unwrap();
        assert_eq!(d.id, "immich_server");
        assert_eq!(d.status, ContainerStatus::Running);
        assert_eq!(
            d.started_at,
            "2026-04-08T19:46:34.627081493Z"
                .parse::<Timestamp>()
                .unwrap()
        );
        assert_eq!(d.finished_at, None);
        assert_eq!(d.restart_policy, RestartPolicy::Always);
        assert_eq!(
            d.port_bindings,
            vec![PortBinding {
                container_port: 2283,
                host_port: 12080,
                protocol: Protocol::Tcp
            }]
        );
        assert_eq!(
            d.networks
                .iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>(),
            ["immich_default", "monitoring"]
        );
        assert_eq!(
            d.volume_bindings,
            vec![
                VolumeMount {
                    source: "/etc/localtime".into(),
                    destination: "/etc/localtime".into(),
                    mode: MountMode::Ro
                },
                VolumeMount {
                    source: "/docker/immich/upload".into(),
                    destination: "/data".into(),
                    mode: MountMode::Rw
                }
            ]
        );
        assert_eq!(d.env_variables.len(), 5);
        assert_eq!(d.env_variables[3].key, "DB_PASSWORD");
        assert_eq!(d.entrypoint, ["tini", "--", "/bin/bash", "-c"]);
        assert_eq!(d.cmd, ["start.sh"]);
        assert_eq!(d.labels["com.docker.compose.project"], "immich");
        assert_eq!(d.resources.memory_bytes, 394334208);
        assert_eq!(
            (d.memory_limit, d.exit_code, d.oom_killed, d.privileged),
            (0, 0, false, false)
        );
    }

    #[test]
    fn stopped_container_has_finished_at() {
        let mut api = fixtures();
        let state = &mut api.details.get_mut("immich_server").unwrap().details.state;
        state.running = false;
        state.finished_at = "2026-04-09T10:00:00Z".into();
        let d = svc(api).get_container("immich_server").unwrap();
        assert_eq!(d.status, ContainerStatus::Stopped);
        assert_eq!(d.finished_at, Some("2026-04-09T10:00:00Z".parse().unwrap()));
    }

    #[test]
    fn zero_finished_at_is_omitted() {
        let mut api = fixtures();
        api.details
            .get_mut("immich_server")
            .unwrap()
            .details
            .state
            .running = false;
        let d = svc(api).get_container("immich_server").unwrap();
        assert_eq!(d.finished_at, None);
    }

    #[test]
    fn status_precedence() {
        let s = |f: fn(&mut ContainerState)| {
            let mut st = ContainerState::default();
            f(&mut st);
            container_status(&st)
        };
        assert_eq!(
            s(|st| {
                st.dead = true;
                st.running = true
            }),
            ContainerStatus::Dead
        );
        assert_eq!(
            s(|st| {
                st.restarting = true;
                st.paused = true
            }),
            ContainerStatus::Restarting
        );
        assert_eq!(
            s(|st| {
                st.paused = true;
                st.running = true
            }),
            ContainerStatus::Paused
        );
        assert_eq!(s(|st| st.running = true), ContainerStatus::Running);
        assert_eq!(s(|_| {}), ContainerStatus::Stopped);
    }

    #[test]
    fn restart_policy_maps() {
        assert_eq!(restart_policy("always"), RestartPolicy::Always);
        assert_eq!(restart_policy("no"), RestartPolicy::No);
        assert_eq!(
            restart_policy("unless-stopped"),
            RestartPolicy::UnlessStopped
        );
        assert_eq!(restart_policy("on-failure"), RestartPolicy::OnFailure);
        assert_eq!(restart_policy(""), RestartPolicy::No);
    }

    #[test]
    fn get_unknown_is_not_found() {
        assert_eq!(
            svc(fixtures())
                .get_container("nope")
                .unwrap_err()
                .to_string(),
            "Container not found: nope"
        );
    }

    #[test]
    fn actions_reach_the_api() {
        let api = fixtures();
        let log = api.actions.clone();
        let s = svc(api);
        s.container_action("prometheus", Action::Start).unwrap();
        s.container_action("prometheus", Action::Stop).unwrap();
        s.container_action("prometheus", Action::Restart).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            ["start prometheus", "stop prometheus", "restart prometheus"]
        );
        assert_eq!(Action::Restart.past(), "Restarted");
    }

    #[test]
    fn json_shape() {
        let v =
            serde_json::to_value(svc(fixtures()).get_container("immich_server").unwrap()).unwrap();
        assert_eq!(v["restartPolicy"], "always");
        assert_eq!(v["portBindings"][0]["protocol"], "tcp");
        assert_eq!(v["volumeBindings"][0]["mode"], "ro");
        assert_eq!(v["resources"]["cpuPercent"], 0.325);
        assert_eq!(v["startedAt"], "2026-04-08T19:46:34.627081493Z");
        assert!(v.get("finishedAt").is_none());
        assert!(v.get("device").is_none() && v.get("uri").is_none());
    }
}
