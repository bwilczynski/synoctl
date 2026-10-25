//! `backups logs`: the Hyper Backup log, read like `journalctl`. Assigns
//! each entry to a task, pages through the log, filters and orders it, and
//! follows it.

use std::collections::HashMap;
use std::time::Duration;

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

use super::Service;
use crate::dsm::BackupLogQuery;
use crate::dsm::types::{BackupLogRecord, BackupTask};
use crate::error::{Error, Result};
use crate::model::backup_log::{BackupLogEntry, LogLevel, LogTask};

/// Largest page requested from the NAS.
pub(crate) const PAGE_SIZE: usize = 1000;

/// What `backups logs` asks for.
#[derive(Debug, Clone, Default)]
pub struct BackupLogFilter {
    /// A task ID; `None` means every task, and entries without one.
    pub task: Option<String>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    /// `None` means every level.
    pub min_level: Option<LogLevel>,
    /// Keep only the newest N matching entries.
    pub lines: Option<usize>,
    pub newest_first: bool,
}

impl BackupLogFilter {
    fn matches(&self, e: &BackupLogEntry) -> bool {
        self.min_level.is_none_or(|min| e.level >= min)
            && self
                .task
                .as_ref()
                .is_none_or(|id| e.task.as_ref().is_some_and(|t| same_task_id(&t.id, id)))
    }
}

/// Task IDs are numbers, so `03` names task `3`, as in `backups get`.
fn same_task_id(a: &str, b: &str) -> bool {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A DSM log time (`YYYY/MM/DD HH:MM:SS`, NAS zone) as an instant. A time
/// that is ambiguous or skipped by a DST change resolves to the earlier one.
pub(crate) fn nas_time(s: &str, tz: &TimeZone) -> Option<Timestamp> {
    let dt = DateTime::strptime("%Y/%m/%d %H:%M:%S", s).ok()?;
    tz.to_ambiguous_zoned(dt)
        .compatible()
        .ok()
        .map(|z| z.timestamp())
}

/// An entry's task, and its message without the prefix that named it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Resolved {
    pub task: Option<LogTask>,
    pub message: String,
    /// `[Local][X]`/`[Network][X]` with an `X` the task list lacks: a task
    /// created since the list was fetched, or one deleted since.
    pub unknown_named: bool,
}

/// The task list, for naming the task each entry belongs to.
pub(crate) struct TaskIndex {
    tasks: Vec<BackupTask>,
    pub(crate) fetched_at: Timestamp,
}

impl TaskIndex {
    pub(crate) fn new(tasks: Vec<BackupTask>, fetched_at: Timestamp) -> Self {
        Self { tasks, fetched_at }
    }

    /// `Backup task not found: <id>` unless a task has this ID.
    fn require(&self, id: &str) -> Result<()> {
        let known = id
            .parse::<i64>()
            .is_ok_and(|n| self.tasks.iter().any(|t| t.task_id == n));
        if known {
            Ok(())
        } else {
            Err(Error::NotFound(format!("Backup task not found: {id}")))
        }
    }

    fn by_name(&self, name: &str) -> Option<&BackupTask> {
        self.tasks.iter().find(|t| t.name == name)
    }

    /// The task writing to target `id`. Tasks can share a target (a local
    /// and a remote copy of the same data); this NAS logs rotations only for
    /// its local target, so a lone local task wins. Otherwise none.
    fn target_owner(&self, id: &str) -> Option<&BackupTask> {
        let sharing: Vec<&BackupTask> = self.tasks.iter().filter(|t| t.target_id == id).collect();
        if let [only] = sharing.as_slice() {
            return Some(only);
        }
        match sharing
            .iter()
            .filter(|t| t.transfer_type.starts_with("image_local"))
            .collect::<Vec<_>>()
            .as_slice()
        {
            [only] => Some(only),
            _ => None,
        }
    }

    pub(crate) fn resolve(&self, event: &str) -> Resolved {
        let found = |t: &BackupTask, message: &str| Resolved {
            task: Some(LogTask {
                id: t.task_id.to_string(),
                name: t.name.clone(),
            }),
            message: message.to_string(),
            unknown_named: false,
        };
        let unchanged = |unknown_named| Resolved {
            task: None,
            message: event.to_string(),
            unknown_named,
        };
        let origin = event
            .strip_prefix("[Local]")
            .or_else(|| event.strip_prefix("[Network]"));
        if let Some((name, rest)) = origin.and_then(bracket) {
            return match self.by_name(name) {
                Some(t) => found(t, rest),
                None => unchanged(true),
            };
        }
        if let Some((name, rest)) = bracket(event)
            && let Some(t) = self.by_name(name)
        {
            return found(t, rest);
        }
        let target =
            bracketed_after(event, "from ID [").or_else(|| bracketed_after(event, "from target ["));
        if let Some(t) = target.and_then(|id| self.target_owner(id)) {
            return found(t, event);
        }
        if let Some(t) = bracketed_after(event, "from task [").and_then(|n| self.by_name(n)) {
            return found(t, event);
        }
        unchanged(false)
    }

    fn entry(&self, time: Timestamp, r: &BackupLogRecord) -> (BackupLogEntry, bool) {
        let resolved = self.resolve(&r.event);
        let entry = BackupLogEntry {
            time,
            level: LogLevel::from_dsm(&r.level),
            task: resolved.task,
            user: r.user.clone(),
            message: resolved.message,
        };
        (entry, resolved.unknown_named)
    }
}

/// `[name] rest` → (`name`, `rest`).
fn bracket(s: &str) -> Option<(&str, &str)> {
    let (name, rest) = s.strip_prefix('[')?.split_once(']')?;
    Some((name, rest.trim_start()))
}

/// The text between `marker` (ending in `[`) and the next `]`.
fn bracketed_after<'a>(s: &'a str, marker: &str) -> Option<&'a str> {
    let start = s.find(marker)? + marker.len();
    s[start..].split_once(']').map(|(inside, _)| inside)
}

/// How far back `--follow` looks again on every poll. DSM writes some
/// entries late (an integrity check's "finished" can carry a time half an
/// hour old), and NAS times in the repeated hour when DST ends parse an hour
/// early.
pub(crate) const OVERLAP_SECS: i64 = 3600;

/// Where `--follow` continues: the newest second fetched so far (matching
/// or not), and how many of each entry were fetched in the `OVERLAP_SECS`
/// before it. Entries have no ID, so identical ones are told apart only by
/// count.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Cursor {
    pub(crate) second: i64,
    pub(crate) seen: HashMap<(i64, BackupLogRecord), usize>,
}

impl Cursor {
    /// After the first fetch; at `now` when it returned nothing.
    pub(crate) fn start(fetched: &[(Timestamp, BackupLogRecord)], now: Timestamp) -> Self {
        let second = fetched
            .iter()
            .map(|(t, _)| t.as_second())
            .max()
            .unwrap_or(now.as_second());
        let mut cursor = Self {
            second,
            seen: HashMap::new(),
        };
        cursor.remember(fetched);
        cursor
    }

    /// The second polls start from.
    pub(crate) fn from(&self) -> i64 {
        self.second - OVERLAP_SECS
    }

    fn remember(&mut self, records: &[(Timestamp, BackupLogRecord)]) {
        if let Some(newest) = records.iter().map(|(t, _)| t.as_second()).max() {
            self.second = self.second.max(newest);
        }
        let from = self.from();
        for (t, r) in records.iter().filter(|(t, _)| t.as_second() >= from) {
            *self.seen.entry((t.as_second(), r.clone())).or_default() += 1;
        }
        self.seen.retain(|(s, _), _| *s >= from);
    }

    /// The entries of `batch` (fetched from `self.from()` on) not fetched
    /// before, in `batch`'s order; then moves past them.
    pub(crate) fn take_fresh(
        &mut self,
        batch: Vec<(Timestamp, BackupLogRecord)>,
    ) -> Vec<(Timestamp, BackupLogRecord)> {
        let from = self.from();
        let mut skip = self.seen.clone();
        let fresh: Vec<_> = batch
            .into_iter()
            .filter(|(t, r)| {
                if t.as_second() < from {
                    return false;
                }
                match skip.get_mut(&(t.as_second(), r.clone())) {
                    Some(n) if *n > 0 => {
                        *n -= 1;
                        false
                    }
                    _ => true,
                }
            })
            .collect();
        self.remember(&fresh);
        fresh
    }
}

/// Entries for one `backups logs` run, plus what `--follow` needs.
pub struct BackupLogPage {
    pub entries: Vec<BackupLogEntry>,
    pub(crate) tasks: TaskIndex,
    pub(crate) cursor: Cursor,
}

/// `entries` arrive newest first as DSM lists them. Oldest first by time;
/// entries with the same time keep the reverse of DSM's order. Then the
/// newest `lines`, and newest first if asked.
fn order(entries: &mut Vec<BackupLogEntry>, lines: Option<usize>, newest_first: bool) {
    entries.reverse();
    entries.sort_by_key(|e| e.time);
    if let Some(n) = lines {
        let drop = entries.len().saturating_sub(n);
        entries.drain(..drop);
    }
    if newest_first {
        entries.reverse();
    }
}

/// Pause between polls while following.
pub const FOLLOW_INTERVAL: Duration = Duration::from_secs(5);
/// Longest pause between retries after failures.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How stale the task list may get before an unknown task name refetches it.
const TASKS_REFRESH: Duration = Duration::from_secs(60);

/// Continues a `backups logs` run: each poll returns the matching entries
/// that arrived since the previous one.
pub struct Follower {
    filter: BackupLogFilter,
    tasks: TaskIndex,
    pub(crate) cursor: Cursor,
}

impl BackupLogPage {
    /// The entries to print first, and a follower that continues after them.
    pub fn follow(self, filter: BackupLogFilter) -> (Vec<BackupLogEntry>, Follower) {
        let follower = Follower {
            filter,
            tasks: self.tasks,
            cursor: self.cursor,
        };
        (self.entries, follower)
    }
}

impl Follower {
    /// New matching entries, oldest first.
    pub fn poll(&mut self, svc: &Service, now: Timestamp) -> Result<Vec<BackupLogEntry>> {
        // Fetch every entry in the overlap before the cursor, so it moves
        // past entries the filter drops too; never before `--since` (the
        // cursor sits at the first fetch's `now` when that came back empty).
        let from = match self.filter.since {
            Some(since) => self.cursor.from().max(since.as_second()),
            None => self.cursor.from(),
        };
        // `to` pinned to `now`, so pages do not shift while DSM adds entries;
        // later ones come with the next poll.
        let query = BackupLogQuery {
            offset: 0,
            limit: PAGE_SIZE,
            from: Some(from),
            to: Some(now.as_second()),
        };
        let fresh = self.cursor.take_fresh(svc.fetch_log(query, |_, _| false)?);
        let mut entries: Vec<(BackupLogEntry, bool)> =
            fresh.iter().map(|(t, r)| self.tasks.entry(*t, r)).collect();
        if entries.iter().any(|(_, unknown)| *unknown) && self.refresh_tasks(svc, now) {
            entries = fresh.iter().map(|(t, r)| self.tasks.entry(*t, r)).collect();
        }
        let mut entries: Vec<BackupLogEntry> = entries
            .into_iter()
            .map(|(e, _)| e)
            .filter(|e| self.filter.matches(e))
            .collect();
        order(&mut entries, None, false);
        Ok(entries)
    }

    /// Refetches the task list, at most once per `TASKS_REFRESH`. A failed
    /// refetch keeps the old list. Whether the list changed.
    fn refresh_tasks(&mut self, svc: &Service, now: Timestamp) -> bool {
        let age = now.duration_since(self.tasks.fetched_at);
        if age < jiff::SignedDuration::try_from(TASKS_REFRESH).expect("fits") {
            return false;
        }
        match svc.api.backup_tasks() {
            Ok(list) => {
                self.tasks = TaskIndex::new(list.task_list, now);
                true
            }
            Err(_) => {
                self.tasks.fetched_at = now;
                false
            }
        }
    }
}

/// Delay before the next poll after `failures` consecutive failures.
pub fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        return FOLLOW_INTERVAL;
    }
    (FOLLOW_INTERVAL * 2u32.pow(failures.min(5) - 1)).min(MAX_BACKOFF)
}

/// Polls until the reader goes away: sleeps, polls, hands each non-empty
/// batch to `emit` (which returns false when stdout is closed). An
/// authentication failure ends it with that error; other failures are
/// retried with [`backoff`], warning once per streak.
pub fn follow_loop(
    mut poll: impl FnMut() -> Result<Vec<BackupLogEntry>>,
    mut emit: impl FnMut(&[BackupLogEntry]) -> Result<bool>,
    mut sleep: impl FnMut(Duration),
    mut warn: impl FnMut(&str),
) -> Result<()> {
    let mut failures = 0u32;
    loop {
        sleep(backoff(failures));
        match poll() {
            Ok(entries) => {
                failures = 0;
                if !entries.is_empty() && !emit(&entries)? {
                    return Ok(());
                }
            }
            Err(e @ Error::Auth(_)) => return Err(e),
            Err(e) => {
                failures = failures.saturating_add(1);
                if failures == 1 {
                    warn(&format!("NAS unreachable, retrying: {e}"));
                }
            }
        }
    }
}

impl Service {
    /// The log entries `f` asks for, up to `now` unless `f.until` says
    /// otherwise.
    pub fn backup_logs(&self, f: &BackupLogFilter, now: Timestamp) -> Result<BackupLogPage> {
        let tasks = TaskIndex::new(self.api.backup_tasks()?.task_list, now);
        if let Some(id) = &f.task {
            tasks.require(id)?;
        }
        let query = BackupLogQuery {
            offset: 0,
            limit: PAGE_SIZE,
            from: f.since.map(|t| t.as_second()),
            to: Some(f.until.unwrap_or(now).as_second()),
        };
        let mut entries = Vec::new();
        let mut newest = i64::MIN;
        // Stop once N entries match and the pages read cover the follow
        // overlap, so `--follow`'s cursor knows every entry it will re-read.
        let fetched = self.fetch_log(query, |time, r| {
            newest = newest.max(time.as_second());
            let (e, _) = tasks.entry(time, r);
            if f.matches(&e) {
                entries.push(e);
            }
            f.lines.is_some_and(|n| entries.len() >= n) && newest - time.as_second() > OVERLAP_SECS
        })?;
        order(&mut entries, f.lines, f.newest_first);
        Ok(BackupLogPage {
            entries,
            cursor: Cursor::start(&fetched, now),
            tasks,
        })
    }

    /// Reads pages from `query.offset` until the last one, or until `enough`
    /// (called per entry) says so at the end of a page. Entries whose time
    /// does not parse are skipped. Returns every entry read, in DSM's order.
    fn fetch_log(
        &self,
        mut query: BackupLogQuery,
        mut enough: impl FnMut(Timestamp, &BackupLogRecord) -> bool,
    ) -> Result<Vec<(Timestamp, BackupLogRecord)>> {
        let mut fetched = Vec::new();
        loop {
            let page = self.api.backup_log(&query)?;
            let n = page.log_list.len();
            let mut done = false;
            for r in page.log_list {
                let Some(time) = nas_time(&r.time, &self.tz) else {
                    continue;
                };
                done = enough(time, &r);
                fetched.push((time, r));
            }
            query.offset += n;
            if done || n == 0 || query.offset as i64 >= page.total {
                return Ok(fetched);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use jiff::tz::TimeZone;

    use super::*;
    use crate::dsm::types::{BackupLogRecord, BackupTask};
    use crate::error::Error;
    use crate::model::backup_log::LogLevel;
    use crate::service::testutil::{fixtures, svc};

    fn now() -> Timestamp {
        "2026-04-25T12:00:00Z".parse().unwrap()
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn task(id: i64, name: &str, target: &str, transfer: &str) -> BackupTask {
        BackupTask {
            task_id: id,
            name: name.into(),
            target_id: target.into(),
            transfer_type: transfer.into(),
            ..Default::default()
        }
    }

    fn index() -> TaskIndex {
        let tasks = fixtures().backup_tasks.unwrap().task_list;
        TaskIndex::new(tasks, now())
    }

    fn record(time: &str, level: &str, event: &str) -> BackupLogRecord {
        BackupLogRecord {
            event: event.into(),
            level: level.into(),
            time: time.into(),
            user: "SYSTEM".into(),
        }
    }

    fn task_id(e: &BackupLogEntry) -> Option<&str> {
        e.task.as_ref().map(|t| t.id.as_str())
    }

    #[test]
    fn resolve_local_and_network_prefix() {
        let r = index().resolve("[Network][Backup to REMOTE] Backup task started.");
        assert_eq!(r.task.unwrap().id, "4");
        assert_eq!(r.message, "Backup task started.");
        assert!(!r.unknown_named);
        let r = index().resolve("[Local][Backup to LOCAL] Backup task started.");
        assert_eq!(r.task.unwrap().name, "Backup to LOCAL");
    }

    #[test]
    fn resolve_bare_task_prefix() {
        let r = index().resolve("[Backup to LOCAL] Trigger version rotation.");
        assert_eq!(r.task.unwrap().id, "3");
        assert_eq!(r.message, "Trigger version rotation.");
    }

    #[test]
    fn resolve_unknown_task_after_local() {
        let ev = "[Local][Backup to OLD] Backup task finished successfully.";
        let r = index().resolve(ev);
        assert_eq!(r.task, None);
        assert_eq!(r.message, ev);
        assert!(r.unknown_named);
    }

    #[test]
    fn resolve_shared_target_goes_to_local_task() {
        let ev = "[backup] Version rotation completed from ID [host-01_1.hbk].";
        let r = index().resolve(ev);
        assert_eq!(r.task.unwrap().id, "3");
        assert_eq!(r.message, ev);
        assert!(!r.unknown_named);
    }

    #[test]
    fn resolve_shared_target_between_two_local_tasks_is_ambiguous() {
        let idx = TaskIndex::new(
            vec![
                task(1, "A", "x.hbk", "image_local"),
                task(2, "B", "x.hbk", "image_local"),
            ],
            now(),
        );
        let r = idx.resolve("[backup] Rotate version [2026-01-01 00:00:00] from ID [x.hbk].");
        assert_eq!(r.task, None);
        assert!(!r.unknown_named);
    }

    #[test]
    fn resolve_unique_target() {
        let idx = TaskIndex::new(vec![task(7, "R", "y.hbk", "image_remote")], now());
        let r = idx.resolve(
            "Deleted version from target [y.hbk] at backup destination [/volume1/backup].",
        );
        assert_eq!(r.task.unwrap().id, "7");
    }

    #[test]
    fn resolve_from_task() {
        let ev = "Ready to delete version [2026-03-11 01:00:22] from task [Backup to LOCAL].";
        let r = index().resolve(ev);
        assert_eq!(r.task.unwrap().id, "3");
        assert_eq!(r.message, ev);
    }

    #[test]
    fn level_from_dsm() {
        assert_eq!(LogLevel::from_dsm("info"), LogLevel::Info);
        assert_eq!(LogLevel::from_dsm("warn"), LogLevel::Warning);
        assert_eq!(LogLevel::from_dsm("err"), LogLevel::Error);
        assert_eq!(LogLevel::from_dsm("debug"), LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warning && LogLevel::Warning < LogLevel::Error);
    }

    #[test]
    fn nas_time_in_zone() {
        let warsaw = TimeZone::get("Europe/Warsaw").unwrap();
        assert_eq!(
            nas_time("2026/10/25 01:16:42", &warsaw),
            Some(ts("2026-10-24T23:16:42Z"))
        );
        // The repeated hour when DST ends: the earlier instant (CEST).
        assert_eq!(
            nas_time("2026/10/25 02:30:00", &warsaw),
            Some(ts("2026-10-25T00:30:00Z"))
        );
        assert_eq!(nas_time("garbage", &warsaw), None);
    }

    fn logs(f: &BackupLogFilter) -> Vec<BackupLogEntry> {
        svc(fixtures()).backup_logs(f, now()).unwrap().entries
    }

    #[test]
    fn default_is_newest_lines_oldest_first() {
        let e = logs(&BackupLogFilter {
            lines: Some(2),
            ..Default::default()
        });
        let msgs: Vec<&str> = e.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "Data integrity check finished. 1.2 GB data checked this time.",
                "Backup integrity check is finished. No error was found.",
            ]
        );
    }

    #[test]
    fn newest_first_reverses() {
        let e = logs(&BackupLogFilter {
            lines: Some(2),
            newest_first: true,
            ..Default::default()
        });
        assert_eq!(e[0].time, ts("2026-04-25T09:06:49Z"));
        assert_eq!(e[1].time, ts("2026-04-25T05:30:24Z"));
    }

    #[test]
    fn same_second_entries_keep_reverse_nas_order() {
        let e = logs(&BackupLogFilter {
            min_level: Some(LogLevel::Warning),
            ..Default::default()
        });
        let times: Vec<Timestamp> = e.iter().map(|e| e.time).collect();
        assert_eq!(
            times,
            [
                ts("2026-04-24T21:49:38Z"),
                ts("2026-04-24T21:50:01Z"),
                ts("2026-04-25T01:16:48Z"),
                ts("2026-04-25T01:16:48Z"),
            ]
        );
        assert!(e[2].message.ends_with("db.sock]"));
        assert!(e[3].message.ends_with("app.sock]"));
        assert_eq!(e[0].level, LogLevel::Error);
        assert_eq!(e[2].level, LogLevel::Warning);
    }

    #[test]
    fn out_of_order_nas_entries_are_sorted() {
        let e = logs(&BackupLogFilter {
            task: Some("3".into()),
            ..Default::default()
        });
        let i = |t: &str| e.iter().position(|e| e.time == ts(t)).unwrap();
        assert!(i("2026-04-25T02:00:28Z") < i("2026-04-25T02:30:13Z"));
        assert!(e.windows(2).all(|w| w[0].time <= w[1].time));
    }

    #[test]
    fn task_filter_keeps_only_that_task() {
        let e = logs(&BackupLogFilter {
            task: Some("4".into()),
            ..Default::default()
        });
        assert_eq!(e.len(), 4);
        assert!(e.iter().all(|e| task_id(e) == Some("4")));
    }

    #[test]
    fn no_task_filter_includes_unassigned() {
        let e = logs(&BackupLogFilter::default());
        assert_eq!(e.len(), 16);
        let old = e
            .iter()
            .find(|e| e.time == ts("2026-04-24T19:00:00Z"))
            .unwrap();
        assert_eq!(old.task, None);
        assert_eq!(old.user, "SYSTEM");
    }

    #[test]
    fn unknown_task_is_not_found() {
        let f = BackupLogFilter {
            task: Some("99".into()),
            ..Default::default()
        };
        let Err(e) = svc(fixtures()).backup_logs(&f, now()) else {
            panic!("expected an error");
        };
        assert!(matches!(e, Error::NotFound(_)));
        assert_eq!(e.to_string(), "Backup task not found: 99");
    }

    #[test]
    fn window_is_inclusive() {
        let e = logs(&BackupLogFilter {
            since: Some(ts("2026-04-24T20:00:00Z")),
            until: Some(ts("2026-04-24T21:49:38Z")),
            ..Default::default()
        });
        assert_eq!(e.len(), 2);
    }

    #[test]
    fn reads_every_page_until_total() {
        let mut api = fixtures();
        api.backup_log = (0..2500)
            .map(|i| {
                record(
                    &format!("2026/04/25 10:{:02}:{:02}", 59 - i / 60 % 60, 59 - i % 60),
                    "info",
                    "[Network][Backup to REMOTE] Backup task started.",
                )
            })
            .collect();
        api.backup_log.push(record(
            "2026/04/24 08:00:00",
            "info",
            "[Local][Backup to LOCAL] Backup task started.",
        ));
        let f = BackupLogFilter {
            task: Some("3".into()),
            lines: Some(50),
            ..Default::default()
        };
        let e = svc(api).backup_logs(&f, now()).unwrap().entries;
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].time, ts("2026-04-24T08:00:00Z"));
    }

    #[test]
    fn empty_log_prints_nothing() {
        let mut api = fixtures();
        api.backup_log.clear();
        let page = svc(api)
            .backup_logs(&BackupLogFilter::default(), now())
            .unwrap();
        assert!(page.entries.is_empty());
        assert_eq!(page.cursor.second, now().as_second());
        assert!(page.cursor.seen.is_empty());
    }

    #[test]
    fn first_page_cursor_counts_the_newest_second() {
        let page = svc(fixtures())
            .backup_logs(
                &BackupLogFilter {
                    min_level: Some(LogLevel::Error),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        // The cursor follows fetched entries, not printed ones.
        assert_eq!(page.cursor.second, ts("2026-04-25T09:06:49Z").as_second());
        assert_eq!(page.cursor.seen.values().sum::<usize>(), 1);
    }

    #[test]
    fn unparseable_time_is_dropped() {
        let mut api = fixtures();
        api.backup_log = vec![
            record("bogus", "info", "[Local][Backup to LOCAL] A."),
            record("2026/04/25 01:00:00", "info", "[Local][Backup to LOCAL] B."),
        ];
        let e = svc(api)
            .backup_logs(&BackupLogFilter::default(), now())
            .unwrap()
            .entries;
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].message, "B.");
    }

    // --- following ---

    use crate::dsm::fixtures::FixtureApi;
    use crate::dsm::types::BackupTaskList;
    use std::time::Duration;

    /// `now()` plus `secs`, as a DSM log time (the fixture NAS is UTC).
    fn at(secs: i64) -> String {
        let t = Timestamp::from_second(now().as_second() + secs).unwrap();
        t.to_zoned(TimeZone::UTC)
            .strftime("%Y/%m/%d %H:%M:%S")
            .to_string()
    }

    fn later(secs: i64) -> Timestamp {
        Timestamp::from_second(now().as_second() + secs).unwrap()
    }

    /// DSM lists newest first: new records go to the front.
    fn push(api: &mut FixtureApi, r: BackupLogRecord) {
        api.backup_log.insert(0, r);
    }

    fn start(api: &FixtureApi, filter: BackupLogFilter) -> Follower {
        let page = svc(api.clone()).backup_logs(&filter, now()).unwrap();
        page.follow(filter).1
    }

    fn poll(f: &mut Follower, api: &FixtureApi, at: Timestamp) -> Vec<BackupLogEntry> {
        f.poll(&svc(api.clone()), at).unwrap()
    }

    fn local(time: &str, msg: &str) -> BackupLogRecord {
        record(time, "info", &format!("[Local][Backup to LOCAL] {msg}"))
    }

    #[test]
    fn follow_returns_only_new_entries_once() {
        let mut api = fixtures();
        let mut f = start(&api, BackupLogFilter::default());
        push(&mut api, local(&at(1), "New."));
        let got = poll(&mut f, &api, later(5));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "New.");
        assert!(poll(&mut f, &api, later(10)).is_empty());
    }

    #[test]
    fn follow_first_page_is_returned_by_follow() {
        let api = fixtures();
        let filter = BackupLogFilter {
            lines: Some(10),
            ..Default::default()
        };
        let page = svc(api).backup_logs(&filter, now()).unwrap();
        let (first, _) = page.follow(filter);
        assert_eq!(first.len(), 10);
    }

    #[test]
    fn same_second_entries_are_not_repeated() {
        let mut api = fixtures();
        let mut f = start(&api, BackupLogFilter::default());
        // The newest fixture entry's second, 09:06:49.
        push(&mut api, local("2026/04/25 09:06:49", "Same second."));
        let got = poll(&mut f, &api, later(5));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "Same second.");
        assert!(poll(&mut f, &api, later(10)).is_empty());
    }

    #[test]
    fn two_identical_entries_in_one_second_both_print() {
        let mut api = fixtures();
        let mut f = start(&api, BackupLogFilter::default());
        push(&mut api, local(&at(3), "Twin."));
        push(&mut api, local(&at(3), "Twin."));
        assert_eq!(poll(&mut f, &api, later(5)).len(), 2);
        assert!(poll(&mut f, &api, later(10)).is_empty());
        push(&mut api, local(&at(3), "Twin."));
        assert_eq!(poll(&mut f, &api, later(15)).len(), 1);
    }

    #[test]
    fn cursor_moves_past_unmatched_entries() {
        let mut api = fixtures();
        let mut f = start(
            &api,
            BackupLogFilter {
                min_level: Some(LogLevel::Error),
                ..Default::default()
            },
        );
        push(&mut api, local(&at(10), "Fine."));
        assert!(poll(&mut f, &api, later(15)).is_empty());
        assert_eq!(f.cursor.second, later(10).as_second());
    }

    #[test]
    fn empty_log_cursor_starts_at_now() {
        let mut api = fixtures();
        api.backup_log.clear();
        let mut f = start(&api, BackupLogFilter::default());
        push(&mut api, local(&at(1), "First."));
        let got = poll(&mut f, &api, later(5));
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn poll_never_starts_before_since() {
        let mut api = fixtures();
        api.backup_log.clear();
        let mut f = start(
            &api,
            BackupLogFilter {
                since: Some(later(60)),
                ..Default::default()
            },
        );
        push(&mut api, local(&at(30), "Too early."));
        push(&mut api, local(&at(90), "In window."));
        let got = poll(&mut f, &api, later(95));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "In window.");
    }

    fn add_task(api: &mut FixtureApi, id: i64, name: &str) {
        let list: &mut BackupTaskList = api.backup_tasks.as_mut().unwrap();
        list.task_list.push(task(id, name, "z.hbk", "image_local"));
    }

    #[test]
    fn unknown_task_refetches_task_list_once_a_minute() {
        let mut api = fixtures();
        let mut f = start(&api, BackupLogFilter::default());
        add_task(&mut api, 5, "Backup to NEW");
        push(
            &mut api,
            record(
                &at(61),
                "info",
                "[Local][Backup to NEW] Backup task started.",
            ),
        );
        let got = poll(&mut f, &api, later(61));
        assert_eq!(task_id(&got[0]), Some("5"));
        assert_eq!(got[0].message, "Backup task started.");

        add_task(&mut api, 6, "Backup to NEWER");
        push(
            &mut api,
            record(
                &at(62),
                "info",
                "[Local][Backup to NEWER] Backup task started.",
            ),
        );
        let got = poll(&mut f, &api, later(62));
        assert_eq!(got[0].task, None);

        push(
            &mut api,
            record(
                &at(122),
                "info",
                "[Local][Backup to NEWER] Backup task finished.",
            ),
        );
        let got = poll(&mut f, &api, later(122));
        assert_eq!(task_id(&got[0]), Some("6"));
    }

    #[test]
    fn backoff_sequence() {
        let secs: Vec<u64> = (0..7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [5, 5, 10, 20, 40, 60, 60]);
    }

    fn entry_at(secs: i64) -> BackupLogEntry {
        BackupLogEntry {
            time: later(secs),
            level: LogLevel::Info,
            task: None,
            user: "SYSTEM".into(),
            message: "x".into(),
        }
    }

    #[test]
    fn follow_loop_warns_once_per_streak_and_stops_on_auth() {
        let dsm = || Error::Dsm("synology request: timed out".into());
        let mut script = vec![
            Err(dsm()),
            Err(dsm()),
            Ok(vec![entry_at(1)]),
            Err(dsm()),
            Err(Error::Auth("login failed".into())),
        ]
        .into_iter();
        let (mut sleeps, mut warnings, mut batches) = (Vec::new(), Vec::new(), 0);
        let r = follow_loop(
            || script.next().unwrap(),
            |_| {
                batches += 1;
                Ok(true)
            },
            |d: Duration| sleeps.push(d.as_secs()),
            |w: &str| warnings.push(w.to_string()),
        );
        assert!(matches!(r, Err(Error::Auth(_))));
        assert_eq!(sleeps, [5, 5, 10, 5, 5]);
        assert_eq!(batches, 1);
        assert_eq!(
            warnings,
            [
                "NAS unreachable, retrying: synology request: timed out",
                "NAS unreachable, retrying: synology request: timed out",
            ]
        );
    }

    #[test]
    fn follow_loop_ends_when_reader_is_gone() {
        let r = follow_loop(|| Ok(vec![entry_at(1)]), |_| Ok(false), |_| {}, |_| {});
        assert!(r.is_ok());
    }

    #[test]
    fn late_written_entry_within_the_overlap_is_printed() {
        let mut api = fixtures();
        let mut f = start(&api, BackupLogFilter::default());
        // Written after the newest fixture entry (09:06:49) but stamped
        // half an hour earlier, as DSM's "Data integrity check finished" is.
        push(&mut api, local("2026/04/25 08:36:49", "Late."));
        let got = poll(&mut f, &api, later(5));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "Late.");
        assert!(poll(&mut f, &api, later(10)).is_empty());
    }

    #[test]
    fn overlap_does_not_repeat_older_entries() {
        let mut api = fixtures();
        let mut f = start(&api, BackupLogFilter::default());
        push(&mut api, local(&at(1), "New."));
        assert_eq!(poll(&mut f, &api, later(5)).len(), 1);
        // The fixture's 09:06:49 entry is within an hour of nothing new;
        // nothing already fetched comes back.
        for t in [10, 15, 3600] {
            assert!(poll(&mut f, &api, later(t)).is_empty(), "at +{t}s");
        }
    }

    #[test]
    fn entry_after_poll_time_waits_for_the_next_poll() {
        let mut api = fixtures();
        let mut f = start(&api, BackupLogFilter::default());
        push(&mut api, local(&at(100), "Future."));
        assert!(poll(&mut f, &api, later(5)).is_empty());
        assert_eq!(poll(&mut f, &api, later(105)).len(), 1);
    }

    #[test]
    fn first_page_seeds_the_cursor_with_the_whole_overlap() {
        let mut api = fixtures();
        // More entries in one second than fit on a page, none of them new.
        api.backup_log = (0..1100)
            .map(|i| local("2026/04/25 11:00:00", &format!("Entry {i}.")))
            .collect();
        let filter = BackupLogFilter {
            lines: Some(10),
            ..Default::default()
        };
        let page = svc(api.clone()).backup_logs(&filter, now()).unwrap();
        let (first, mut f) = page.follow(filter);
        assert_eq!(first.len(), 10);
        assert!(poll(&mut f, &api, later(5)).is_empty());
    }

    #[test]
    fn task_id_with_leading_zero_matches() {
        let three = logs(&BackupLogFilter {
            task: Some("3".into()),
            ..Default::default()
        });
        let padded = logs(&BackupLogFilter {
            task: Some("03".into()),
            ..Default::default()
        });
        assert!(!three.is_empty());
        assert_eq!(padded, three);
    }

    #[test]
    fn follow_with_padded_task_id_prints_new_entries() {
        let mut api = fixtures();
        let mut f = start(
            &api,
            BackupLogFilter {
                task: Some("03".into()),
                ..Default::default()
            },
        );
        push(&mut api, local(&at(1), "New."));
        assert_eq!(poll(&mut f, &api, later(5)).len(), 1);
    }
}
