//! Table rendering, ported from hlctl's docker, storage and system
//! templates (homelab-cli/internal/cli/{docker,storage,system}/templates).

pub mod backup_logs;
pub mod backups;
pub mod check;
pub mod docker;
pub mod format;
pub mod info;
pub mod status;
pub mod style;
pub mod utilization;
pub mod volumes;

use std::io::Write;

use tabwriter::TabWriter;

/// Tab-separated rows aligned like Go's `tabwriter.NewWriter(w, 0, 0, 2, ' ', 0)`.
/// `flush()` ends a column block (Go templates' `{{ flush }}`), so sections
/// get independent widths. Cells may contain `ESC[…m` colour codes; they do
/// not count toward width.
pub struct Table {
    tw: TabWriter<Vec<u8>>,
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

// Writes go to an in-memory Vec<u8> and cannot fail.
impl Table {
    pub fn new() -> Self {
        Self {
            tw: TabWriter::new(Vec::new()).minwidth(0).padding(2).ansi(true),
        }
    }

    pub fn row(&mut self, cells: &[&str]) {
        writeln!(self.tw, "{}", cells.join("\t")).expect("write to memory");
    }

    /// A line with no columns, e.g. `--- PORTS ---`.
    pub fn line(&mut self, text: &str) {
        writeln!(self.tw, "{text}").expect("write to memory");
    }

    pub fn blank(&mut self) {
        writeln!(self.tw).expect("write to memory");
    }

    pub fn flush(&mut self) {
        self.tw.flush().expect("write to memory");
    }

    pub fn finish(self) -> String {
        let bytes = self.tw.into_inner().expect("write to memory");
        String::from_utf8(bytes).expect("renderers only write UTF-8")
    }
}

/// `FIELD  VALUE` rows, the layout of every `get`.
pub fn fields(t: &mut Table, rows: &[(&str, String)]) {
    t.row(&["FIELD", "VALUE"]);
    for (k, v) in rows {
        t.row(&[k, v]);
    }
}

/// Starts a titled section below what came before, with its own column widths.
pub fn section(t: &mut Table, title: &str) {
    t.flush();
    t.blank();
    t.line(title);
}

#[cfg(test)]
pub(crate) mod testutil {
    /// Drops `ESC [ … m` sequences.
    pub(crate) fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_columns_with_two_space_padding() {
        let mut t = Table::new();
        t.row(&["ID", "NAME"]);
        t.row(&["a", "longer"]);
        t.row(&["abcd", "x"]);
        assert_eq!(t.finish(), "ID    NAME\na     longer\nabcd  x\n");
    }

    #[test]
    fn flush_starts_independent_block() {
        let mut t = Table::new();
        t.row(&["A", "B"]);
        t.flush();
        t.blank();
        t.line("--- X ---");
        t.row(&["LONGVALUE", "C"]);
        assert_eq!(t.finish(), "A  B\n\n--- X ---\nLONGVALUE  C\n");
    }

    #[test]
    fn escape_codes_do_not_count_toward_width() {
        let mut t = Table::new();
        t.row(&["\x1b[32mup\x1b[0m", "x"]);
        t.row(&["down", "y"]);
        assert_eq!(t.finish(), "\x1b[32mup\x1b[0m    x\ndown  y\n");
    }
}
