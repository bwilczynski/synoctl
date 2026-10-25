# synoctl

A command-line client for Synology NAS: system info and load, storage
volumes and disks, Hyper Backup tasks, and Container Manager containers,
images, networks and image updates, straight from DSM with no server in
between.

```sh
$ synoctl status
SYSTEM      ok    DS918+  DSM 7.1.1-42962 Update 9  up 4d 4h 0m 0s  43°C
VOLUMES     ok    1 volume
DISKS       ok    4 disks, hottest 41°C
CONTAINERS  warn  2 of 3 running
                  stopped: prometheus
BACKUPS     warn  2 tasks
                  warning: Backup to LOCAL
                  result unknown: Backup to REMOTE
UPDATES     ok    0 of 3 up to date, 2 not checked yet, 1 without a release source
```

## Install

Prebuilt binaries for macOS (Apple Silicon and Intel) and Linux (x86_64 and
arm64) are attached to every [GitHub release](https://github.com/bwilczynski/synoctl/releases).
Install them with [mise](https://mise.jdx.dev):

```sh
mise use -g github:bwilczynski/synoctl    # latest release
synoctl --version
```

Append `@<version>` to pin a release; `mise upgrade synoctl` moves to a newer
one. On Linux the binary needs glibc 2.35 or newer.

### From source

```sh
mise install
cargo install --path .
```

## Configure

Run the guided setup; it asks for the NAS address, account and password (or
a command that prints it, see below), offers to accept DSM's self-signed certificate, writes the config file
readable only by you, and checks that everything works:

```
synoctl config init
```

Use a dedicated DSM account for synoctl, without 2-step verification, which
synoctl does not support. DSM shows storage, Hyper Backup and Container
Manager only to administrators, so the account most likely needs to be in the
administrators group; `config check` tells you what it can reach.

For scripts, pass everything as flags (`--host`, `--username`, `--password`
or `--password-command`, `--insecure-tls`); the password can come from
`SYNO_PASSWORD` instead, which keeps it out of shell history. `--force` overwrites an existing file;
`--no-check` skips contacting the NAS.

### Password from a password manager

To keep the password out of the config file, set `password_command` instead
of `password`: a shell command that prints the password on its first line.
synoctl runs it only when it has to log in, which is rarely, since it keeps
the DSM session between runs. The command shares your terminal, so a password
manager can ask to be unlocked. For [pass](https://www.passwordstore.org/):

```toml
password_command = "pass show nas/synoctl"
```

pass keeps the password on an entry's first line, so any lines after it (a
username, notes) are ignored. If gpg-agent asks for the passphrase with a
terminal pinentry, export `GPG_TTY=$(tty)` in your shell. `rbw get synoctl`,
`bw get password synoctl` (needs `BW_SESSION`) and `op read op://…` work the
same way. `SYNO_PASSWORD` still takes precedence over either key.

When something stops working, `synoctl config check` walks through the
config, file permissions, DNS, TCP, TLS, the DSM Web API, login, and which
packages (Container Manager, Hyper Backup) are installed, and says which step
fails and what to change. It exits non-zero on failure.

The file lives at `~/.config/synoctl/config.toml` (or
`$XDG_CONFIG_HOME/synoctl/config.toml`, or `--config PATH`, or
`SYNOCTL_CONFIG`). Every key:

```toml
host = "192.168.1.10:5001"     # host[:port]; always https (DSM's HTTPS port is 5001)
username = "synoctl"
password = "…"                 # or:
# password_command = "pass show nas/synoctl"   # prints the password; run at login
# insecure_tls = true          # DSM ships a self-signed certificate
# timezone = "Europe/Warsaw"   # the NAS's zone, for backup times; default: yours
# auth_version = 6             # SYNO.API.Auth login version; 3 for old DSM
# cache_ttl = 15               # seconds to reuse DSM responses; 0 disables

[updates]
# check_interval = "1h"        # how long release lookups are reused
[[updates.sources]]               # only for images without a source label
image = "grafana/loki"         # image without tag
source = "grafana/loki"
# type = "codeberg"            # default: github
```

Every key but `updates` can be overridden by an environment variable:
`SYNO_HOST`, `SYNO_USERNAME`, `SYNO_PASSWORD`, `SYNO_INSECURE_TLS`,
`SYNO_TIMEZONE`, `SYNO_AUTH_VERSION`, `SYNO_CACHE_TTL`.

DSM records every login, so synoctl keeps its session between runs in
`$XDG_CACHE_HOME/synoctl/session.json` (or `~/.cache/synoctl`), readable only
by you, and logs in again only when DSM ends the session. DSM responses are
cached there for `cache_ttl` seconds, so a `list` followed by a few `get`s
asks the NAS once; pass `--refresh` to any command to skip the cache.

## Use

```
synoctl status
synoctl info
synoctl utilization
synoctl volumes list [--status STATUS] [--wide] | get <id>
synoctl backups list [--status STATUS] [--result RESULT] [--wide] | get <id>
synoctl backups logs [<id>] [-n N] [-S WHEN] [-U WHEN] [-p LEVEL] [-r] [-f]
synoctl docker containers list [--status STATUS | --all] [--wide] | get <name>
synoctl docker containers start|stop|restart <name>
synoctl docker images list | get <id>
synoctl docker networks list | get <name>
synoctl docker updates list|check [--status STATUS | --all] | get <name>
```

Every command takes `-o json`; every `list` also takes `-o ids`, printing one
ID per line for scripts (`synoctl docker containers list --status stopped -o ids
| xargs -n1 synoctl docker containers start`). IDs are DSM's own: container
and network names, the first 12 characters of an image digest, volume IDs
(`volume_1`) and backup task numbers.

`docker containers list` shows running containers and `docker updates list`
shows containers with an update available, unless you pick a `--status` or
pass `--all`. Filters combine: `synoctl backups list --status idle --result
failed` lists idle tasks whose last run failed.

`--wide` adds columns: volume usage and filesystem, a backup task's last and
next run and stored size, a container's memory share. A container's restart
count is only in `get`: DSM's container list does not report it.

On a terminal, status cells are coloured (green healthy, yellow degraded or
warning, red crashed, failing or failed, dim stopped or idle), as are volume
usage and disk temperatures. `--color always|never` overrides that;
`NO_COLOR` turns it off.

### Status

`synoctl status` answers "is anything wrong with the NAS?" on one screen, a
line per area with a verdict and the culprits named below it:

| Area | warn | error |
|---|---|---|
| SYSTEM | DSM reports a temperature warning | |
| VOLUMES | volume or storage pool degraded or repairing, or 80% used | volume or storage pool crashed, or 95% used |
| DISKS | disk status warning, or 50 °C | failing or critical |
| CONTAINERS | a container not running | a dead container |
| BACKUPS | idle task's last run a partial success (`warning`) or not known yet, task suspended | idle task's last run failed or canceled, task in error (authentication failed, crashed) |
| UPDATES | `info` when a container is behind its latest release | |

Backup tasks read as Hyper Backup's own task list does: a busy task shows what
it is doing (`running`, or `checking` during an integrity check, which makes
the row `info`) rather than its last result, and a run that ended `done` is a
success whatever error code DSM attached.

The command exits 1 when a row is `error`. A row whose data cannot be read
says so and shows `unknown` without failing the command; the container and
backup rows are left out when Container Manager or Hyper Backup is not
installed.

### Backup logs

When `backups list` says a task's last run ended in `warning`, `synoctl backups
logs` shows why. It reads Hyper Backup's log the way `journalctl` reads the
journal: the 50 most recent entries, oldest first, one per line, with the task
each belongs to.

```sh
$ synoctl backups logs 3 -p warning --since yesterday
Apr 25 01:16:48 Backup to LOCAL: Incomplete file backup. (The file is a special file.) [/volume1/docker/app/data/db.sock]
Apr 25 01:16:48 Backup to LOCAL: Incomplete file backup. (The file is a special file.) [/volume1/docker/app/data/app.sock]
```

- A task ID keeps that task's entries; without one, every task's entries are
  shown, and entries that name no known task show `-`.
- `-n N` shows the last N entries; `-S`/`--since` and `-U`/`--until` take
  `now`, `today`, `yesterday`, `-2h`, `"2h ago"` or `YYYY-MM-DD [HH:MM[:SS]]`
  in local time, and without `-n` show the whole window.
- `-p info|warning|error` keeps that level and above; on a terminal, warnings
  are yellow and errors red.
- `-r` puts the newest first; `-f` keeps running and prints new entries as they
  arrive (checking every 5 seconds).
- `-o json` prints one JSON object per line.

The NAS writes log times in its own zone; set `timezone` in the config when it
differs from yours.

### Docker updates

`synoctl docker updates` compares each container's image tag with the latest
release of its source project, found in this order:

1. an `[[updates.sources]]` entry;
2. the image's `org.opencontainers.image.source` label, when it points at
   github.com or codeberg.org;
3. for `ghcr.io/<owner>/<repo>` images, that GitHub repository.

Most images carry the label, so few need an entry; synoctl names the ones that
do. Containers on `latest` or pinned to a digest (even with a tag, as in
`app:1.2@sha256:…`) are not tracked.

Tags are compared as versions (`v2.6.10` is newer than `2.6.3`, and a
`-rc.1` pre-release comes before its release), so a container ahead of the
latest release is `upToDate`, not offered a downgrade. Tags that differ but
are not both versions, such as `stable` against `v1.0.0`, are `incomparable`.

DSM shows labels only in a container's details, so synoctl reads each
container's, eight at a time, and keeps what they say in
`~/.cache/synoctl/sources.json` by container ID. A container is only read
again once it is recreated, as pulling a new image does, or on
`docker updates check`.

Release lookups are kept in `~/.cache/synoctl/releases.json` for
`check_interval` (an hour by default); `docker updates check` looks again
now. GitHub allows 60 unauthenticated lookups an hour: set `GITHUB_TOKEN` if
you track more images than that.

## Shell completions

`synoctl completions <bash|zsh|fish|powershell|elvish>` prints a script that
completes subcommands, flags and IDs (containers, images, networks, volumes,
backup tasks). Load it from your shell's startup file:

```sh
source <(synoctl completions bash)              # ~/.bashrc
source <(synoctl completions zsh)               # ~/.zshrc, after compinit
synoctl completions fish | source               # ~/.config/fish/config.fish
```

IDs come from the NAS through the response cache, waiting at most 3 seconds;
completion reads the config from `SYNOCTL_CONFIG` or the default path.

## Relationship to hlctl

synoctl is a port of `hlctl docker|storage|system …`
([bwilczynski/homelab-cli](https://github.com/bwilczynski/homelab-cli)) and
the Synology parts of [bwilczynski/homelab-api](https://github.com/bwilczynski/homelab-api),
with its output model taken from
[bwilczynski/homelab-api-spec](https://github.com/bwilczynski/homelab-api-spec),
the same way [unifictl](https://github.com/bwilczynski/unifictl) replaced
`hlctl network`. The differences:

- One NAS per config, so IDs have no device prefix and there is no `device`
  field or `DEVICE` column; a second NAS uses a second config file.
- No `uri` field.
- An unreachable NAS is an error, not an empty list.
- `status` replaces `system health`; there is no `--watch`.
- Docker commands live under `synoctl docker`.

## Develop

```sh
mise run test   # unit and CLI tests
mise run lint   # clippy + rustfmt
```

Tests run against sanitized DSM responses in `tests/fixtures` (no NAS
needed); `synoctl --fixtures tests/fixtures <command>` runs any command
against them in a `--features fixtures` build.

## Release

Releases are cut from [conventional commits](https://www.conventionalcommits.org)
(PR titles and commits are checked by commitlint):

1. On every push to `main`, [release-plz](https://release-plz.dev) opens or
   updates a release PR that bumps the version in `Cargo.toml` and adds the
   changes to `CHANGELOG.md`.
2. Merging that PR tags `vX.Y.Z`.
3. The tag triggers [dist](https://opensource.axo.dev/cargo-dist/)
   (`.github/workflows/release.yml`), which builds each target on a native
   runner and publishes the archives and checksums as a GitHub release.

release-plz needs a `RELEASE_PLZ_TOKEN` repository secret: a fine-grained PAT
with *Contents* and *Pull requests* read/write on this repository.
`release.yml` is generated: change `dist-workspace.toml` and run
`dist generate` (dist is pinned in `mise.toml`).
