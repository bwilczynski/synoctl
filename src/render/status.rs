//! `synoctl status`: per line an area, its verdict and a summary, with
//! what is wrong on the lines below. Layout from `unifictl status`.

use super::{Table, style};
use crate::model::{NasStatus, StatusArea};

pub fn table(status: &NasStatus, color: bool) -> String {
    let mut t = Table::new();
    for row in &status.rows {
        let verdict = style::verdict(row.verdict, color);
        t.row(&[label(row.area), &verdict, &row.summary]);
        for p in &row.problems {
            t.row(&["", "", p]);
        }
    }
    t.finish()
}

fn label(area: StatusArea) -> &'static str {
    match area {
        StatusArea::System => "SYSTEM",
        StatusArea::Volumes => "VOLUMES",
        StatusArea::Disks => "DISKS",
        StatusArea::Containers => "CONTAINERS",
        StatusArea::Backups => "BACKUPS",
        StatusArea::Updates => "UPDATES",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{StatusArea, StatusRow, Verdict};

    fn row(area: StatusArea, verdict: Verdict, summary: &str, problems: &[&str]) -> StatusRow {
        StatusRow {
            area,
            verdict,
            summary: summary.into(),
            problems: problems.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn status() -> NasStatus {
        NasStatus {
            rows: vec![
                row(
                    StatusArea::System,
                    Verdict::Ok,
                    "DS918+  DSM 7.1.1  up 4d",
                    &[],
                ),
                row(
                    StatusArea::Containers,
                    Verdict::Warn,
                    "2 of 3 running",
                    &["stopped: prometheus"],
                ),
                row(
                    StatusArea::Backups,
                    Verdict::Error,
                    "2 tasks",
                    &[
                        "failed: Backup to LOCAL",
                        "result unknown: Backup to REMOTE",
                    ],
                ),
                row(
                    StatusArea::Updates,
                    Verdict::Unknown,
                    "cannot read releases: x",
                    &[],
                ),
            ],
        }
    }

    #[test]
    fn table_names_problems_below() {
        assert_eq!(
            table(&status(), false),
            concat!(
                "SYSTEM      ok       DS918+  DSM 7.1.1  up 4d\n",
                "CONTAINERS  warn     2 of 3 running\n",
                "                     stopped: prometheus\n",
                "BACKUPS     error    2 tasks\n",
                "                     failed: Backup to LOCAL\n",
                "                     result unknown: Backup to REMOTE\n",
                "UPDATES     unknown  cannot read releases: x\n",
            )
        );
    }

    #[test]
    fn colour_paints_verdicts_without_moving_columns() {
        let coloured = table(&status(), true);
        assert!(coloured.contains("\x1b[1;31merror\x1b[0m"), "{coloured}");
        assert_eq!(
            crate::render::testutil::strip_ansi(&coloured),
            table(&status(), false)
        );
    }
}
