//! `info`: hlctl's info.tmpl as a field list (one NAS).

use super::format::{format_bytes, format_uptime};
use super::{Table, fields};
use crate::model::SystemInfo;

pub fn table(i: &SystemInfo) -> String {
    let mut t = Table::new();
    fields(
        &mut t,
        &[
            ("MODEL", i.model.clone()),
            ("FIRMWARE", i.firmware.clone()),
            ("RAM", format_bytes(i.ram_mb * 1024 * 1024)),
            ("UPTIME", format_uptime(i.uptime_seconds)),
        ],
    );
    t.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn info_table() {
        assert_eq!(
            table(&svc(fixtures()).info().unwrap()),
            concat!(
                "FIELD     VALUE\n",
                "MODEL     DS918+\n",
                "FIRMWARE  DSM 7.1.1-42962 Update 9\n",
                "RAM       16.0 GB\n",
                "UPTIME    4d 4h 0m 0s\n",
            )
        );
    }
}
