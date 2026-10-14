//! `docker containers`: hlctl's containers_list.tmpl and
//! containers_get.tmpl, without DEVICE and NAME (the ID is the name).

use jiff::tz::TimeZone;

use crate::model::docker::{ContainerDetail, ContainerList};
use crate::render::format::{format_bytes, format_time};
use crate::render::{Table, fields, section, style};

fn cpu(pct: f64) -> String {
    format!("{pct:.1}%")
}

pub fn list(l: &ContainerList, wide: bool, color: bool) -> String {
    let mut t = Table::new();
    let mut header = vec!["ID", "IMAGE", "STATUS", "CPU", "MEMORY"];
    if wide {
        header.extend(["RESTARTS", "MEM%"]);
    }
    t.row(&header);
    for c in &l.items {
        let mut row = vec![
            c.id.clone(),
            c.image.clone(),
            style::container_status(c.status, color),
            cpu(c.resources.cpu_percent),
            format_bytes(c.resources.memory_bytes),
        ];
        if wide {
            row.push(c.restart_count.to_string());
            row.push(cpu(c.resources.memory_percent));
        }
        t.row(&row.iter().map(String::as_str).collect::<Vec<_>>());
    }
    t.finish()
}

pub fn get(d: &ContainerDetail, tz: &TimeZone, color: bool) -> String {
    let mut rows = vec![
        ("ID", d.id.clone()),
        ("STATUS", style::container_status(d.status, color)),
        ("IMAGE", d.image.clone()),
        ("RESTART COUNT", d.restart_count.to_string()),
        ("CPU", cpu(d.resources.cpu_percent)),
        (
            "MEMORY",
            format!(
                "{} ({})",
                format_bytes(d.resources.memory_bytes),
                cpu(d.resources.memory_percent)
            ),
        ),
        ("STARTED AT", format_time(d.started_at, tz)),
    ];
    if let Some(at) = d.finished_at {
        rows.push(("FINISHED AT", format_time(at, tz)));
    }
    rows.extend([
        ("EXIT CODE", d.exit_code.to_string()),
        ("OOM KILLED", d.oom_killed.to_string()),
        ("RESTART POLICY", d.restart_policy.as_str().to_string()),
        ("PRIVILEGED", d.privileged.to_string()),
        (
            "MEMORY LIMIT",
            if d.memory_limit > 0 {
                format_bytes(d.memory_limit)
            } else {
                "unlimited".into()
            },
        ),
    ]);
    let mut t = Table::new();
    fields(&mut t, &rows);
    if !d.port_bindings.is_empty() {
        section(&mut t, "PORT BINDINGS");
        t.row(&["CONTAINER PORT", "HOST PORT", "PROTOCOL"]);
        for p in &d.port_bindings {
            t.row(&[
                &p.container_port.to_string(),
                &p.host_port.to_string(),
                p.protocol.as_str(),
            ]);
        }
    }
    if !d.networks.is_empty() {
        section(&mut t, "NETWORKS");
        t.row(&["NAME", "DRIVER"]);
        for n in &d.networks {
            t.row(&[&n.name, &n.driver]);
        }
    }
    if !d.volume_bindings.is_empty() {
        section(&mut t, "VOLUME BINDINGS");
        t.row(&["SOURCE", "DESTINATION", "MODE"]);
        for v in &d.volume_bindings {
            t.row(&[&v.source, &v.destination, v.mode.as_str()]);
        }
    }
    if !d.env_variables.is_empty() {
        section(&mut t, "ENVIRONMENT VARIABLES");
        t.row(&["KEY", "VALUE"]);
        for e in &d.env_variables {
            t.row(&[&e.key, &e.value]);
        }
    }
    for (title, items) in [("ENTRYPOINT", &d.entrypoint), ("COMMAND", &d.cmd)] {
        if !items.is_empty() {
            section(&mut t, title);
            for item in items {
                t.line(&format!("  {item}"));
            }
        }
    }
    if !d.labels.is_empty() {
        section(&mut t, "LABELS");
        t.row(&["KEY", "VALUE"]);
        for (k, v) in &d.labels {
            t.row(&[k, v]);
        }
    }
    t.finish()
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::*;
    use crate::render::testutil::strip_ansi;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_table() {
        let l = svc(fixtures()).list_containers(None).unwrap();
        assert_eq!(
            list(&l, false, false),
            concat!(
                "ID                       IMAGE                                              STATUS   CPU   MEMORY\n",
                "immich_server            ghcr.io/immich-app/immich-server:v2.6.3            running  0.3%  376.1 MB\n",
                "immich_machine_learning  ghcr.io/immich-app/immich-machine-learning:v2.6.3  running  0.0%  13.4 MB\n",
                "prometheus               prom/prometheus:v3.2.1                             stopped  0.4%  343.0 MB\n",
            )
        );
    }

    #[test]
    fn wide_list_adds_restarts_and_memory_share() {
        let l = svc(fixtures()).list_containers(None).unwrap();
        let out = list(&l, true, false);
        assert!(out.starts_with("ID  "), "{out}");
        assert!(
            out.lines()
                .next()
                .unwrap()
                .ends_with("MEMORY    RESTARTS  MEM%"),
            "{out}"
        );
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .ends_with("376.1 MB  0         2.4%"),
            "{out}"
        );
    }

    #[test]
    fn get_table() {
        let d = svc(fixtures()).get_container("immich_server").unwrap();
        let out = get(&d, &TimeZone::UTC, false);
        assert!(
            out.starts_with(concat!(
                "FIELD           VALUE\n",
                "ID              immich_server\n",
                "STATUS          running\n",
                "IMAGE           ghcr.io/immich-app/immich-server:v2.6.3\n",
                "RESTART COUNT   0\n",
                "CPU             0.3%\n",
                "MEMORY          376.1 MB (2.4%)\n",
                "STARTED AT      2026-04-08 19:46\n",
                "EXIT CODE       0\n",
                "OOM KILLED      false\n",
                "RESTART POLICY  always\n",
                "PRIVILEGED      false\n",
                "MEMORY LIMIT    unlimited\n",
                "\n",
                "PORT BINDINGS\n",
                "CONTAINER PORT  HOST PORT  PROTOCOL\n",
                "2283            12080      tcp\n",
                "\n",
                "NETWORKS\n",
                "NAME            DRIVER\n",
                "immich_default  bridge\n",
                "monitoring      bridge\n",
            )),
            "{out}"
        );
        assert!(out.contains("\nVOLUME BINDINGS\nSOURCE                 DESTINATION     MODE\n/etc/localtime         /etc/localtime  ro\n"), "{out}");
        assert!(out.contains("\nENVIRONMENT VARIABLES\nKEY  "), "{out}");
        assert!(
            out.contains("\nENTRYPOINT\n  tini\n  --\n  /bin/bash\n  -c\n"),
            "{out}"
        );
        assert!(out.contains("\nCOMMAND\n  start.sh\n"), "{out}");
        assert!(out.ends_with("\nLABELS\nKEY                         VALUE\ncom.docker.compose.project  immich\ncom.docker.compose.service  immich-server\n"), "{out}");
    }

    #[test]
    fn colour_paints_without_moving_columns() {
        let l = svc(fixtures()).list_containers(None).unwrap();
        let coloured = list(&l, true, true);
        assert!(coloured.contains("\x1b[2mstopped\x1b[0m"), "{coloured}");
        assert_eq!(strip_ansi(&coloured), list(&l, true, false));
    }
}
