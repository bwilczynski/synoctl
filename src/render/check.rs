//! `config check` report: one row per step, hints indented under the detail.

use super::Table;
use super::style;
use crate::setup::check::Report;

pub fn table(report: &Report, color: bool) -> String {
    let mut t = Table::new();
    for c in &report.checks {
        let mark = style::check_mark(c.status, color);
        t.row(&[&mark, c.step, &c.detail]);
        if let Some(hint) = &c.hint {
            t.row(&["", "", &format!("hint: {hint}")]);
        }
    }
    t.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::check::{Check, Report, Status};

    #[test]
    fn marks_and_hint_lines() {
        let r = Report::new(vec![
            Check::new("config", Status::Pass, "/p"),
            Check::new("tls", Status::Fail, "certificate not trusted: X").hint("h"),
            Check::new("auth", Status::Skip, "skipped: tls failed"),
            Check::new("permissions", Status::Warn, "0644"),
        ]);
        assert_eq!(
            table(&r, false),
            "✓  config       /p\n\
             ✗  tls          certificate not trusted: X\n\
             \x20               hint: h\n\
             –  auth         skipped: tls failed\n\
             !  permissions  0644\n"
        );
    }

    #[test]
    fn coloured_marks_keep_alignment() {
        let r = Report::new(vec![
            Check::new("config", Status::Pass, "/p"),
            Check::new("tls", Status::Fail, "x"),
        ]);
        assert_eq!(
            table(&r, true),
            "\x1b[32m✓\x1b[0m  config  /p\n\x1b[31m✗\x1b[0m  tls     x\n"
        );
    }
}
