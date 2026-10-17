//! `backups`: hlctl's backups_list.tmpl and backups_get.tmpl, without DEVICE.

use jiff::tz::TimeZone;

use super::format::{format_bytes, format_time};
use super::{Table, fields, style};
use crate::model::{BackupTask, BackupTaskDetail, BackupTaskList};

const HEADER: [&str; 5] = ["ID", "NAME", "STATUS", "LAST RESULT", "TYPE"];

fn cells(t: &BackupTask, color: bool) -> Vec<String> {
    vec![
        t.id.clone(),
        t.name.clone(),
        style::backup_status(t.status, color),
        style::backup_result(t.last_result, color),
        t.kind.clone(),
    ]
}

pub fn list(l: &BackupTaskList, color: bool) -> String {
    let mut t = Table::new();
    t.row(&HEADER);
    for task in &l.items {
        let row = cells(task, color);
        t.row(&row.iter().map(String::as_str).collect::<Vec<_>>());
    }
    t.finish()
}

/// `backups list --wide`: every task with its runs and size.
pub fn list_wide(l: &[BackupTaskDetail], tz: &TimeZone, color: bool) -> String {
    let dash = || "-".to_string();
    let mut t = Table::new();
    let mut header = HEADER.to_vec();
    header.extend(["LAST RUN", "NEXT RUN", "SIZE"]);
    t.row(&header);
    for d in l {
        let mut row = cells(d, color);
        row.push(d.last_run_at.map_or_else(dash, |at| format_time(at, tz)));
        row.push(d.next_run_at.map_or_else(dash, |at| format_time(at, tz)));
        row.push(d.size.map_or_else(dash, format_bytes));
        t.row(&row.iter().map(String::as_str).collect::<Vec<_>>());
    }
    t.finish()
}

pub fn get(d: &BackupTaskDetail, tz: &TimeZone, color: bool) -> String {
    let mut rows = vec![
        ("ID", d.id.clone()),
        ("NAME", d.name.clone()),
        ("STATUS", style::backup_status(d.status, color)),
        ("LAST RESULT", style::backup_result(d.last_result, color)),
        ("TYPE", d.kind.clone()),
    ];
    if let Some(at) = d.last_run_at {
        rows.push(("LAST RUN", format_time(at, tz)));
    }
    if let Some(at) = d.next_run_at {
        rows.push(("NEXT RUN", format_time(at, tz)));
    }
    if let Some(size) = d.size {
        rows.push(("SIZE", format_bytes(size)));
    }
    if let Some(folders) = &d.folders {
        rows.push(("FOLDERS", folders.join(", ")));
    }
    let mut t = Table::new();
    fields(&mut t, &rows);
    t.finish()
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::*;
    use crate::render::testutil::strip_ansi;
    use crate::service::BackupFilter;
    use crate::service::testutil::{fixtures, svc};

    #[test]
    fn list_table() {
        let l = svc(fixtures())
            .list_backups(&BackupFilter::default())
            .unwrap()
            .data;
        assert_eq!(
            list(&l, false),
            concat!(
                "ID  NAME              STATUS  LAST RESULT  TYPE\n",
                "3   Backup to LOCAL   idle    warning      hyperBackup\n",
                "4   Backup to REMOTE  idle    unknown      hyperBackup\n",
            )
        );
    }

    #[test]
    fn wide_list_adds_runs_and_size() {
        let l = svc(fixtures())
            .list_backups_wide(&BackupFilter::default())
            .unwrap()
            .data;
        assert_eq!(
            list_wide(&l, &TimeZone::UTC, false),
            concat!(
                "ID  NAME              STATUS  LAST RESULT  TYPE         LAST RUN          NEXT RUN          SIZE\n",
                "3   Backup to LOCAL   idle    warning      hyperBackup  2026-04-24 02:30  2026-04-25 01:00  3.0 GB\n",
                "4   Backup to REMOTE  idle    unknown      hyperBackup  -                 -                 -\n",
            )
        );
    }

    #[test]
    fn get_table() {
        let d = svc(fixtures()).get_backup("3").unwrap().data;
        assert_eq!(
            get(&d, &TimeZone::UTC, false),
            concat!(
                "FIELD        VALUE\n",
                "ID           3\n",
                "NAME         Backup to LOCAL\n",
                "STATUS       idle\n",
                "LAST RESULT  warning\n",
                "TYPE         hyperBackup\n",
                "LAST RUN     2026-04-24 02:30\n",
                "NEXT RUN     2026-04-25 01:00\n",
                "SIZE         3.0 GB\n",
                "FOLDERS      /volume1/docker, /volume1/Documents\n",
            )
        );
    }

    #[test]
    fn get_without_details_omits_rows() {
        let d = svc(fixtures()).get_backup("4").unwrap().data;
        let out = get(&d, &TimeZone::UTC, false);
        assert!(!out.contains("LAST RUN") && !out.contains("SIZE") && !out.contains("FOLDERS"));
    }

    #[test]
    fn colour_paints_without_moving_columns() {
        let l = svc(fixtures())
            .list_backups(&BackupFilter::default())
            .unwrap()
            .data;
        let coloured = list(&l, true);
        assert!(coloured.contains("\x1b[33mwarning\x1b[0m"), "{coloured}");
        assert!(coloured.contains("\x1b[2munknown\x1b[0m"), "{coloured}");
        assert_eq!(strip_ansi(&coloured), list(&l, false));
    }
}
