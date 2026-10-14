//! `utilization`: hlctl's utilization.tmpl as a field list, then the
//! per-interface and per-disk numbers it left to JSON.

use super::format::{format_bytes, format_bytes_per_sec};
use super::{Table, fields, section};
use crate::model::SystemUtilization;

pub fn table(u: &SystemUtilization) -> String {
    let (c, m) = (&u.cpu, &u.memory);
    let swap_pct = if m.swap_total_bytes > 0 {
        m.swap_used_bytes * 100 / m.swap_total_bytes
    } else {
        0
    };
    let mut t = Table::new();
    fields(
        &mut t,
        &[
            (
                "CPU",
                format!(
                    "{}% (user {}%, system {}%)",
                    c.total_percent, c.user_percent, c.system_percent
                ),
            ),
            (
                "MEMORY",
                format!(
                    "{}% of {} ({} available)",
                    m.used_percent,
                    format_bytes(m.total_bytes),
                    format_bytes(m.available_bytes)
                ),
            ),
            (
                "SWAP",
                format!("{swap_pct}% of {}", format_bytes(m.swap_total_bytes)),
            ),
        ],
    );
    if !u.network.is_empty() {
        section(&mut t, "NETWORK");
        t.row(&["INTERFACE", "RX", "TX"]);
        for n in &u.network {
            t.row(&[
                &n.name,
                &format_bytes_per_sec(n.rx_bytes_per_sec),
                &format_bytes_per_sec(n.tx_bytes_per_sec),
            ]);
        }
    }
    if !u.disks.is_empty() {
        section(&mut t, "DISKS");
        t.row(&["DISK", "READ OPS/S", "WRITE OPS/S"]);
        for d in &u.disks {
            t.row(&[
                &d.name,
                &d.read_ops_per_sec.to_string(),
                &d.write_ops_per_sec.to_string(),
            ]);
        }
    }
    t.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn utilization_table() {
        let u = svc(fixtures())
            .utilization("2026-04-18T10:00:00Z".parse().unwrap())
            .unwrap();
        assert_eq!(
            table(&u),
            concat!(
                "FIELD   VALUE\n",
                "CPU     14% (user 8%, system 2%)\n",
                "MEMORY  46% of 15.5 GB (1.5 GB available)\n",
                "SWAP    18% of 11.3 GB\n",
                "\n",
                "NETWORK\n",
                "INTERFACE  RX         TX\n",
                "bond0      10.4 KB/s  5.3 KB/s\n",
                "\n",
                "DISKS\n",
                "DISK  READ OPS/S  WRITE OPS/S\n",
                "sda   6           30\n",
                "sdb   3           27\n",
                "sdc   8           29\n",
                "sdd   7           29\n",
            )
        );
    }

    #[test]
    fn no_swap_is_zero_percent() {
        let mut api = fixtures();
        api.utilization.memory.total_swap = 0;
        api.utilization.memory.avail_swap = 0;
        let u = svc(api)
            .utilization("2026-04-18T10:00:00Z".parse().unwrap())
            .unwrap();
        assert!(table(&u).contains("SWAP    0% of 0 B\n"));
    }
}
