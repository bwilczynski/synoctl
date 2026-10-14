//! `docker networks`, from homelab-api/internal/docker/networks_service.go.

use crate::dsm::types::Network;
use crate::error::{Error, Result};
use crate::model::docker::{DockerNetwork, DockerNetworkDetail, DockerNetworkList};
use crate::service::Service;

impl Service {
    pub fn list_networks(&self) -> Result<DockerNetworkList> {
        let items = self.api.networks()?.network.iter().map(network).collect();
        Ok(DockerNetworkList { items })
    }

    pub fn get_network(&self, name: &str) -> Result<DockerNetworkDetail> {
        let networks = self.api.networks()?.network;
        let n = networks
            .iter()
            .find(|n| n.name == name)
            .ok_or_else(|| Error::NotFound(format!("Network not found: {name}")))?;
        let present = |s: &str| (!s.is_empty()).then(|| s.to_string());
        Ok(DockerNetworkDetail {
            network: network(n),
            driver: n.driver.clone(),
            subnet: present(&n.subnet),
            gateway: present(&n.gateway),
            ip_range: present(&n.iprange),
            containers: n.containers.clone(),
        })
    }
}

fn network(n: &Network) -> DockerNetwork {
    DockerNetwork {
        id: n.name.clone(),
        name: n.name.clone(),
        connected_containers: n.containers.len() as i64,
    }
}

#[cfg(test)]
mod tests {
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_networks() {
        let l = svc(fixtures()).list_networks().unwrap();
        assert_eq!(
            l.items
                .iter()
                .map(|n| (n.id.as_str(), n.connected_containers))
                .collect::<Vec<_>>(),
            [
                ("vaultwarden_default", 1),
                ("immich_default", 4),
                ("grafana_default", 4),
                ("host", 1)
            ]
        );
    }

    #[test]
    fn get_network_detail() {
        let d = svc(fixtures()).get_network("immich_default").unwrap();
        assert_eq!(
            (
                d.driver.as_str(),
                d.subnet.as_deref(),
                d.gateway.as_deref(),
                d.ip_range.as_deref()
            ),
            ("bridge", Some("172.18.0.0/16"), Some("172.18.0.1"), None)
        );
        assert_eq!(d.containers.len(), 4);
        assert_eq!(d.connected_containers, 4);
    }

    #[test]
    fn host_network_omits_empty_fields() {
        let v = serde_json::to_value(svc(fixtures()).get_network("host").unwrap()).unwrap();
        assert!(
            v.get("subnet").is_none() && v.get("gateway").is_none() && v.get("ipRange").is_none()
        );
        assert_eq!(v["connectedContainers"], 1);
    }

    #[test]
    fn network_not_found() {
        assert_eq!(
            svc(fixtures()).get_network("x").unwrap_err().to_string(),
            "Network not found: x"
        );
    }
}
