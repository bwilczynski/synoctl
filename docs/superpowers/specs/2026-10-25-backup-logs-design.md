# `synoctl backups logs`: the Hyper Backup log (design)

Date: 2026-10-25
Status: draft, awaiting review

## Why

`backups list` says a task's last run ended in `warning`, but not why. Today
the reason is only in Hyper Backup's log in the DSM web UI. On the author's
NAS it is eight "Incomplete file backup. (The file is a special file.)"
entries every night, one per GitLab socket file. `backups logs` shows that
log from the terminal.

It reads like `unifictl logs`, which reads like `journalctl`: same flags
where they translate, one line per entry, `-f` to follow. That design
(`../unifictl/docs/superpowers/specs/2026-10-11-logs-design.md`) is the
reference; this document says where synoctl follows it and where it differs.

Success looks like:

- `synoctl backups logs 3 -p warning --since yesterday` shows which files the
  last night's run skipped.
- `synoctl backups logs` shows the 50 most recent entries of every task,
  oldest at the top.
- `synoctl backups logs -f` prints new entries while a backup runs.
- `synoctl backups logs -o json | jq` gives one object per entry.

## Non-goals

- Other DSM logs (Log Center, Container Manager, …). The command lives under
  `backups` and covers only Hyper Backup.
- Free-text search (`-g`): pipe to `grep`.
- Hyper Backup Vault's server-side log (`SYNO.SDS.Backup.Server.Common.Log`).
- Collapsing repeated entries (the same eight warnings every night).
- Changing a task (excluding the socket files) from synoctl.

The original design's non-goal "scheduled tasks and backup logs
(`ListScheduledTasks`, `ListBackupLogs` … are not ported)" in
`2026-10-14-synoctl-design.md` is amended to drop backup logs.

## Decisions

| Question | Decision |
|---|---|
| Command | `synoctl backups logs [TASK]`, next to `list` and `get` |
| Endpoint | `SYNO.SDS.Backup.Client.Common.Log` `list` v1 |
| Time window | Sent to the NAS (`filter_date_from`, `filter_date_to`) |
| Level and task filter | Applied in synoctl; the NAS cannot filter by level, and ignores `task_id` |
| `--follow` | Polling every 5 s, as unifictl |
| Text format | journalctl `short`: time, task name, message; level shown by colour |
| JSON | JSON Lines, synoctl's own model |
| Time parsing | `src/timespec.rs` copied from unifictl (synoctl already has `jiff`) |
| Caching | Never; log requests bypass the response cache |

## The DSM API

Observed on 2026-10-25 against the author's NAS (DSM 7, Hyper Backup with two
tasks: one local, one network), using an existing session ID.

`GET /webapi/entry.cgi?api=SYNO.SDS.Backup.Client.Common.Log&method=list&version=1`
plus:

| Parameter | Behaviour |
|---|---|
| `offset`, `limit` | Paging, newest first. `limit=5000` works in one call; past the end returns the remainder |
| `filter_date_from`, `filter_date_to` | Epoch seconds, both inclusive. Either may be given alone. A non-integer is error 120 (`{"name": "filter_date_to", "reason": "type"}`) |
| `filter_keyword` | Substring match on the message. Not used (see Filtering) |
| `task_id` | Ignored: every task's entries come back |
| `filter_level` | Accepted as a string but has no effect (`warn`, `err`, `"warn"`); a number or array is error 120. Not used |

Other names tried with no effect: `level`, `keyword`, `datefrom`/`dateto`,
`date_from`/`date_to`, `filter_time_from`, `sort_by`/`sort_direction`,
`task_name`, `filter_task_id`.

Response:

```json
{"data": {
  "total": 7167,
  "error_count": 249,
  "info_count": 3677,
  "log_list": [
    {"event": "[Local][Backup to BUFFALO] Incomplete file backup. (The file is a special file.) [/volume1/docker/gitlab/data/redis/redis.socket]",
     "level": "warn", "time": "2026/10/25 01:16:42", "user": "SYSTEM"}
  ]
}, "success": true}
```

- `total` is the count matching the date filter; it is what paging stops on.
- `error_count` and `info_count` do not add up to `total` (warnings are not
  counted). Ignored.
- `level` is `info`, `warn` or `err`.
- `time` is `YYYY/MM/DD HH:MM:SS` in the NAS's local time, no offset.
- Entries have no ID.
- Order is newest first, but not strictly: 2 of 5000 adjacent pairs were out
  of order (an integrity check "finished" logged before its "started"). The
  displayed order is synoctl's sort, not the NAS's.
- `user` is `SYSTEM` for task events and the DSM user for actions they
  started (version rotation).

On a NAS without Hyper Backup the API does not exist (code 102), which maps to
the existing `NotInstalled("Hyper Backup is not installed on {host}")`.
`call_error` applies that mapping only to `SYNO.Backup.*`, so it is extended
to `SYNO.SDS.Backup.*`.

### Which task an entry belongs to

The message names the task in a prefix. Observed forms, by frequency in 7167
entries:

| Form | Example |
|---|---|
| `[Local][<task>] …`, `[Network][<task>] …` | `[Network][Backup to SNAIL] Backup task started.` |
| `[<task>] …` | `[Backup to BUFFALO] Trigger version rotation.` |
| `[backup] … from ID [<target_id>].` | `[backup] Rotate version [2026-09-27 01:00:24] from ID [buffalo_1.hbk].` |
| Neither (2 entries) | `Ready to delete version [2026-09-11 01:00:22] from task [Backup to BUFFALO].` |

Resolution, in order:

1. A leading `[Local]` or `[Network]` followed by `[<name>]` where `<name>`
   is a task in the task list: that task; both brackets are removed from the
   message. If `<name>` is not a known task, the entry has no task and the
   message is left unchanged.
2. Else a leading `[<name>]` that is a task name: that task; the bracket is
   removed.
3. Else `from ID [<x>]` or `from target [<x>]`: the task that owns target
   `<x>` (see below). Message kept as is.
4. Else `from task [<name>]` with a known name: that task. Message kept.
5. Else no task, message unchanged.

Task names are matched exactly against the current task list. An entry from
a deleted or renamed task has no task. (`[backup]` in form 3 is not a task
name. It is the destination share, so it stays in the message.)

**Owner of a target.** A `target_id` does not identify one task: on the
author's NAS, `Backup to BUFFALO` (`image_local`) and `Backup to SNAIL`
(`image_remote`) both have `target_id` `buffalo_1.hbk` and share `backup`.
The fixture tasks share `host-01_1.hbk` the same way. The `[backup]`
rotation entries are written by this NAS for its local target. Remote
rotations run on the remote server and are not in this log. In the real log,
all 282 `[backup] Version rotation started` entries come 1 to 7 seconds after
`[Backup to BUFFALO] Trigger version rotation.`, never after SNAIL's. So
the owner of target `<x>` is:

- the only task with `target_id == <x>`, if there is exactly one;
- else the only task with `target_id == <x>` and `transfer_type` starting with
  `image_local`, if there is exactly one;
- else no task (my call: ambiguous entries are shown without a task, not
  guessed).

`target_id` and `transfer_type` are already in the `SYNO.Backup.Task` `list`
response. Both are added to `dsm::types::BackupTask`.

## CLI

```
synoctl backups logs [TASK] [-n N] [-S WHEN] [-U WHEN] [-p LEVEL] [-r] [-f]
                     [-o table|json] [--color …]
```

| Argument | Meaning |
|---|---|
| `TASK` | A task ID, completed as `backups get` completes. Unknown ID: `Backup task not found: <id>`, as `get`. Without it, every task, plus entries with no task |
| `-n, --lines N` | The N most recent matching entries. Default and interplay with `-S`/`-U`/`-f` exactly as unifictl: 50 with no window, all with a window, 10 with `-f` and no window |
| `-S, --since WHEN` | Entries at or after WHEN |
| `-U, --until WHEN` | Entries at or before WHEN |
| `-p, --priority LEVEL` | `info`, `warning` or `error`: that level and above (`info` < `warn` < `err`) |
| `-r, --reverse` | Newest first |
| `-f, --follow` | Keep running, print new entries as they arrive |

As in unifictl:

- `WHEN` takes the forms of unifictl's `timespec.rs` (`now`, `today`,
  `yesterday`, `-2h`, `2h ago`, `YYYY-MM-DD [HH:MM[:SS]]`), in the local time
  zone of the machine running synoctl. `--since` after `--until` is an error.
- `-f` with `-U`, and `-f` with `-r`, are clap usage errors (exit 2).
- `-o ids` gives the existing `-o ids is only supported by list commands`.
- `--refresh` is accepted and has no effect (logs are never cached).
- A closed stdout exits 0 silently (synoctl's `app.rs` already does this).

## Times

Two time zones are involved, as elsewhere in synoctl:

- The NAS writes `time` in its own zone: the existing `timezone` config
  (`SYNO_TIMEZONE`, default the local system zone). Entry times are parsed in
  it, and `-S`/`-U` instants are sent to the NAS as epoch seconds.
- Output is in the zone synoctl displays every other time in (`ctx.tz`, the
  system zone).

A NAS-local time that is ambiguous (the repeated hour when DST ends) is
resolved with jiff's default (`compatible`, the earlier instant). A time in
the skipped hour is resolved the same way. Entries are not dropped for it.

## Fetching

1. `now` is taken once (the existing `now()` in `app.rs`, which honours
   `SYNOCTL_NOW` in fixture builds). `filter_date_to` is `--until` or `now`,
   pinned so pages do not shift while new entries arrive. `filter_date_from`
   is `--since` when given, else omitted.
2. Pages of 1000 are requested at `offset` 0, 1000, … until N matching entries
   are collected or `offset >= total`.
3. Entries are parsed, assigned a task, filtered by level and task, then
   sorted by time (stable, so the NAS's relative order breaks ties) oldest
   first, or newest first with `-r`. With `-n`, the N newest are kept.

The task list (`backup_tasks()`, through the cache like every other read) is
fetched once per run. Without a TASK or `-p`, `-n 50` is one log request.

## `--follow`

As unifictl's `Cursor`, the position is the newest entry *fetched*, matched
or not, so a `-p` or TASK filter never makes polls reach back days:

1. Print the last N entries (default 10) as above. The cursor is the newest
   fetched entry's second, or `now` when nothing was fetched.
2. Every 5 seconds, request entries from one hour before the cursor's
   second up to the poll's `now` (`filter_date_from`, `filter_date_to`, so
   pages do not shift while entries arrive), all pages. The hour of overlap
   catches entries DSM writes late (on the author's NAS, an integrity check's
   "finished" entry carries a time about 30 minutes old) and NAS times in the
   repeated DST hour, which parse an hour early. Entries have no ID, so the
   cursor keeps a count per (second, time, level, user, message) of entries
   fetched within that hour. In a new response, the first that many copies of
   each key are skipped. A count, not a set, means two identical entries in
   the same second both print. Filter, sort, print oldest first, then advance
   the cursor.
3. Flush stdout after every batch.

Failures, ending and injected clock/sleep: as unifictl (one warning per
failure streak, `warning: NAS unreachable, retrying: …`, backoff 5 s, 10 s,
20 s, 40 s, 60 s, …; authentication errors exit 1; Ctrl-C through default
SIGINT handling).

The task list is fetched again when an entry has the `[Local][X]` or
`[Network][X]` form with an unknown `X`, at most once a minute, so a task
created while following gets its name. The entry is resolved again after the
refetch, before it is printed. A bare leading `[X]` never triggers a refetch,
because `[backup]` appears every night.

Following writes and flushes each batch as it arrives, unlike the other
commands, which build one string that `app.rs` writes once. `app.rs` gets a
streaming path for this. A closed stdout ends it with exit 0, as now.

## Text output

```
Oct 25 01:16:42 Backup to BUFFALO: Incomplete file backup. (The file is a special file.) [/volume1/docker/gitlab/data/redis/redis.socket]
Oct 25 01:27:55 Backup to BUFFALO: [backup] Version rotation completed from ID [buffalo_1.hbk].
Oct 25 03:24:16 Backup to SNAIL: Backup task finished successfully. [200389 files scanned] [200389 files unchanged]
```

- Time: `%b %d %H:%M:%S` in the display zone.
- Task: the task name; `-` for no task.
- Message: as resolved above. No truncation or wrapping.
- Colour (existing `--color` rules and `render/style.rs`) on the message:
  `err` → `Red`, `warn` → `Yellow`, `info` → none.
- Nothing printed when nothing matches.

## JSON output

One compact object per line (JSON Lines) for every `backups logs` run, as
unifictl, so `-f` and the plain form have the same shape:

```json
{"time": "2026-10-25T01:16:42+02:00", "level": "warning", "task": {"id": "3", "name": "Backup to BUFFALO"}, "user": "SYSTEM", "message": "Incomplete file backup. (The file is a special file.) [/volume1/docker/gitlab/data/redis/redis.socket]"}
```

| Field | Source |
|---|---|
| `time` | RFC 3339, second precision, offset of the display zone |
| `level` | `info`, `warning`, `error` (the `-p` names) |
| `task` | `{id, name}` with `id` a string as in `backups list`; `null` when no task |
| `user` | `user` |
| `message` | The message with the task prefix removed |

## Code layout

- `src/cli.rs`: `BackupsCommand::Logs(BackupLogsArgs)`, `LogLevelArg`.
- `src/dsm/types.rs`: `BackupLogList { total, log_list }`, `BackupLogEntry
  { event, level, time, user }`; `target_id` on `BackupTask`.
- `src/dsm/mod.rs`, `src/dsm/client.rs`: `backup_log(&BackupLogQuery)` on
  `DsmApi` (`offset`, `limit`, `from: Option<i64>`, `to: Option<i64>`),
  uncached, with the usual error mapping.
- `src/dsm/fixtures.rs`: `dsm-backup-log.json` loaded when present;
  `backup_log` applies the date filter and paging, so the service is tested
  against realistic behaviour.
- `src/timespec.rs`: copied from unifictl.
- `src/service/backup_logs.rs`: task resolution, paging, filtering,
  ordering, the follow loop (clock and sleep injected).
- `src/model`: `BackupLogEntry`, `LogLevel`.
- `src/render/backup_logs.rs`: text line and colour.
- `src/app.rs`: dispatch.
- `README.md`: usage line and a short section.
- The original design's non-goal amended (see Non-goals).

## Testing

Unit tests:

- `timespec`: the unifictl tests come along with the copy.
- Task resolution: each form in the table above, unknown name after
  `[Local]`, `[backup]` by `target_id` with one task, with a local and a
  remote task sharing it (goes to the local one), and with two local tasks
  sharing it (no task); `from task [...]`; deleted task.
- Time parsing: NAS zone to display zone; a DST-ambiguous time.
- Paging: stops at N matches; reads every page when filtering by task or
  level; pinned `filter_date_to`; out-of-order NAS entries sorted.
- Follow: new entries printed once; same-second entries not repeated; two
  identical entries in one second both printed; a repeated identical message
  in a new second is printed; cursor follows fetched (not printed) entries and
  starts at `now` on an empty log; backoff and reset;
  auth error stops; task list refetch rate limit.
- Colour per level; no escapes when uncoloured.

`httpmock` tests in `client.rs`: API, method, version, `offset`/`limit`/date
parameters, cache untouched, code 102 → not installed.

Integration tests (`tests/cli.rs`) with `--fixtures`, `SYNOCTL_NOW` and
`TZ=UTC`: default output, a TASK, unknown TASK, `-p warning`, `-S`/`-U`,
`-r`, `-o json`, rejected flag combinations, `-o ids` error, completion
candidates for TASK.

The fixture `tests/fixtures/dsm-backup-log.json` follows the real log's
structure with the existing fixtures' fictional names: tasks `Backup to
LOCAL` (3, `image_local`) and `Backup to REMOTE` (4, `image_remote`), both
with target `host-01_1.hbk` as in `dsm-backup-tasks.json`, and paths and
user names that are not the author's. It includes at least one entry of every
prefix form, all three levels, an entry with no task, and two entries out of
order.

## Left to the builder

- Module and type names beyond those named above.
- How much of unifictl's follow loop to copy: it is a model, not a
  dependency; nothing is shared between the two crates.
