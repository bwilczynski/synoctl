//! `docker networks`: hlctl's networks_list.tmpl and networks_get.tmpl,
//! without DEVICE and NAME (the ID is the name).

use crate::model::docker::{DockerNetworkDetail, DockerNetworkList};
use crate::render::{Table, fields, section};

pub fn list(l: &DockerNetworkList) -> String {
    let mut t = Table::new();
    t.row(&["ID", "CONTAINERS"]);
    for n in &l.items {
        t.row(&[&n.id, &n.connected_containers.to_string()]);
    }
    t.finish()
}

pub fn get(d: &DockerNetworkDetail) -> String {
    let mut rows = vec![
        ("ID", d.id.clone()),
        ("DRIVER", d.driver.clone()),
        ("CONTAINERS", d.connected_containers.to_string()),
    ];
    for (k, v) in [
        ("SUBNET", &d.subnet),
        ("GATEWAY", &d.gateway),
        ("IP RANGE", &d.ip_range),
    ] {
        if let Some(v) = v {
            rows.push((k, v.clone()));
        }
    }
    let mut t = Table::new();
    fields(&mut t, &rows);
    if !d.containers.is_empty() {
        section(&mut t, "CONNECTED CONTAINERS");
        t.row(&["NAME"]);
        for c in &d.containers {
            t.row(&[c]);
        }
    }
    t.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_table() {
        let l = svc(fixtures()).list_networks().unwrap();
        assert_eq!(
            list(&l),
            concat!(
                "ID                   CONTAINERS\n",
                "vaultwarden_default  1\n",
                "immich_default       4\n",
                "grafana_default      4\n",
                "host                 1\n",
            )
        );
    }

    #[test]
    fn get_table() {
        let d = svc(fixtures()).get_network("vaultwarden_default").unwrap();
        assert_eq!(
            get(&d),
            concat!(
                "FIELD       VALUE\n",
                "ID          vaultwarden_default\n",
                "DRIVER      bridge\n",
                "CONTAINERS  1\n",
                "SUBNET      172.20.0.0/16\n",
                "GATEWAY     172.20.0.1\n",
                "\n",
                "CONNECTED CONTAINERS\n",
                "NAME\n",
                "vaultwarden\n",
            )
        );
    }

    #[test]
    fn host_network_has_no_addresses() {
        let out = get(&svc(fixtures()).get_network("host").unwrap());
        assert!(!out.contains("SUBNET") && !out.contains("GATEWAY"), "{out}");
    }
}
