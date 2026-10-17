//! `volumes`: hlctl's volumes_list.tmpl and volumes_get.tmpl, without the
//! DEVICE and NAME columns (a volume's ID is its name).

use super::format::format_bytes;
use super::{Table, fields, section, style};
use crate::model::{VolumeDetail, VolumeList};

pub fn list(l: &VolumeList, wide: bool, color: bool) -> String {
    let mut t = Table::new();
    let mut header = vec!["ID", "RAID", "STATUS", "SIZE", "USED"];
    if wide {
        header.extend(["USED%", "FILESYSTEM"]);
    }
    t.row(&header);
    for v in &l.items {
        let mut row = vec![
            v.id.clone(),
            v.raid_type.clone(),
            style::volume_status(v.status, color),
            format_bytes(v.total_bytes),
            format_bytes(v.used_bytes),
        ];
        if wide {
            row.push(style::usage(v.used_percent(), color));
            row.push(v.file_system.clone());
        }
        t.row(&row.iter().map(String::as_str).collect::<Vec<_>>());
    }
    t.finish()
}

pub fn get(d: &VolumeDetail, color: bool) -> String {
    let used = match d.used_percent() {
        Some(p) => format!(
            "{} ({})",
            format_bytes(d.used_bytes),
            style::usage(Some(p), color)
        ),
        None => format_bytes(d.used_bytes),
    };
    let mut t = Table::new();
    fields(
        &mut t,
        &[
            ("ID", d.id.clone()),
            ("FILESYSTEM", d.file_system.clone()),
            ("RAID", d.raid_type.clone()),
            ("STATUS", style::volume_status(d.status, color)),
            ("POOL STATUS", style::volume_status(d.pool_status, color)),
            ("MOUNT PATH", d.mount_path.clone()),
            ("SIZE", format_bytes(d.total_bytes)),
            ("USED", used),
        ],
    );
    if !d.disks.is_empty() {
        section(&mut t, "DISKS");
        t.row(&["ID", "MODEL", "STATUS", "TEMP", "SIZE"]);
        for disk in &d.disks {
            t.row(&[
                &disk.id,
                &disk.model,
                &style::disk_status(disk.status, color),
                &style::temperature(disk.temperature_celsius, color),
                &format_bytes(disk.total_bytes),
            ]);
        }
    }
    t.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::testutil::strip_ansi;
    use crate::service::VolumeFilter;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_table() {
        let l = svc(fixtures())
            .list_volumes(&VolumeFilter::default())
            .unwrap();
        assert_eq!(
            list(&l, false, false),
            "ID        RAID    STATUS  SIZE     USED\nvolume_1  single  normal  10.5 TB  7.5 TB\n"
        );
    }

    #[test]
    fn wide_list_adds_usage_and_filesystem() {
        let l = svc(fixtures())
            .list_volumes(&VolumeFilter::default())
            .unwrap();
        assert_eq!(
            list(&l, true, false),
            concat!(
                "ID        RAID    STATUS  SIZE     USED    USED%  FILESYSTEM\n",
                "volume_1  single  normal  10.5 TB  7.5 TB  71.5%  btrfs\n",
            )
        );
    }

    #[test]
    fn get_table() {
        let d = svc(fixtures()).get_volume("volume_1").unwrap();
        assert_eq!(
            get(&d, false),
            concat!(
                "FIELD        VALUE\n",
                "ID           volume_1\n",
                "FILESYSTEM   btrfs\n",
                "RAID         single\n",
                "STATUS       normal\n",
                "POOL STATUS  normal\n",
                "MOUNT PATH   /volume1\n",
                "SIZE         10.5 TB\n",
                "USED         7.5 TB (71.5%)\n",
                "\n",
                "DISKS\n",
                "ID   MODEL                 STATUS  TEMP  SIZE\n",
                "sda  WD Red Plus 4TB       normal  35°C  3.6 TB\n",
                "sdb  WD Red Plus 4TB       normal  41°C  3.6 TB\n",
                "sdc  Seagate IronWolf 4TB  normal  41°C  3.6 TB\n",
                "sdd  Seagate IronWolf 4TB  normal  36°C  3.6 TB\n",
            )
        );
    }

    #[test]
    fn colour_paints_without_moving_columns() {
        let s = svc(fixtures());
        let l = s.list_volumes(&VolumeFilter::default()).unwrap();
        let coloured = list(&l, true, true);
        assert!(coloured.contains("\x1b[32mnormal\x1b[0m"), "{coloured}");
        assert_eq!(strip_ansi(&coloured), list(&l, true, false));
        let d = s.get_volume("volume_1").unwrap();
        let coloured = get(&d, true);
        assert!(coloured.contains("\x1b[32m35°C\x1b[0m"), "{coloured}");
        assert_eq!(strip_ansi(&coloured), get(&d, false));
    }
}
