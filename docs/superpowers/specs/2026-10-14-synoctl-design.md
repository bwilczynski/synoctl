# synoctl — standalone Synology DSM CLI (design)

Date: 2026-10-14
Status: draft, awaiting review

## Goal

A standalone Rust CLI, `synoctl`, that talks directly to a Synology NAS over
the DSM Web API and provides the Synology functionality of `hlctl` — without
running `homelab-api`. It is the Synology counterpart of
[unifictl](https://github.com/bwilczynski/unifictl), built the same way.

Behaviour is ported from:

- [bwilczynski/homelab-api](https://github.com/bwilczynski/homelab-api):
  `internal/adapters/synology.go` (DSM client and raw types),
  `internal/docker/*_service.go`, `internal/storage/*_service.go`,
  `internal/system/{info,utilization,health,updates}_service.go` and
  `internal/system/github.go` — data fetching and mapping
- [bwilczynski/homelab-cli](https://github.com/bwilczynski/homelab-cli) (`hlctl`):
  `internal/cli/{docker,storage,system}/` and their templates — flags and tables
- [bwilczynski/homelab-api-spec](https://github.com/bwilczynski/homelab-api-spec):
  the docker, storage and system schemas — the output model

Infrastructure that is not Synology-specific (response cache, private file
writes, time specs, completion, colour/format helpers, config init/check flow,
release pipeline) is copied from unifictl and adapted, not shared through a
common crate.

### Intentional differences from hlctl / homelab-api

1. **One NAS per config.** No `device` field, no `DEVICE` column, no
   `--device` filter, and no `{device}.` prefix in IDs. A second NAS uses a
   second config file (`--config` / `SYNOCTL_CONFIG`).
2. **No `uri` field** in any output schema.
3. **Unreachable NAS is an error** (exit 1), not an empty list.
4. **No `--watch`.**
5. **`status` replaces `system health`**, with per-area verdicts in the style
   of `unifictl status` (see below). UniFi health is not part of synoctl.
6. **No startup API discovery.** DSM's "API does not exist" error is mapped to
   a "not installed" message instead (see DSM client).
7. **Errors are plain stderr messages**, not RFC 9457 problem documents.

### Non-goals

Multiple NASes per invocation; 2-step verification (OTP) login; scheduled
tasks and backup logs (`ListScheduledTasks`, `ListBackupLogs` are unused in
homelab-api and are not ported); write operations beyond container
start/stop/restart; VHS demo GIFs (after v0.1).

## Commands

Basic NAS commands are flat; Docker (Container Manager) resources are nested
under `docker`.

```
synoctl status
synoctl info
synoctl utilization
synoctl volumes  list [--wide] | get <id>
synoctl backups  list [--wide] | get <id>
synoctl docker containers list [--status STATUS] [--wide]
                        | get <name>
                        | start <name> | stop <name> | restart <name>
synoctl docker images     list | get <id>
synoctl docker networks   list | get <name>
synoctl docker updates    list [--status STATUS] | get <name> | check
synoctl config init | check
synoctl completions <bash|zsh|fish|powershell|elvish>
```

Global flags, as in unifictl: `-o table|json|ids`, `--color auto|always|never`
(and `NO_COLOR`), `--refresh` (skip cached responses), `--config PATH`, and a
hidden `--fixtures DIR` behind the `fixtures` cargo feature for tests.

### IDs

DSM's own identifiers, unprefixed:

| Resource | ID | Example |
|---|---|---|
| container | name | `immich-server` |
| image | first 12 hex chars of the ID, `sha256:` stripped | `3f1a9c2b7d40` |
| Docker network | name | `bridge` |
| volume | DSM volume ID | `volume_1` |
| backup task | DSM numeric task ID | `3` |
| update | container name | `immich-server` |

`-o ids` prints one ID per line on every `list`. IDs complete on TAB for
`get`, `start`, `stop` and `restart`, through the response cache.

### Output

- **Tables** keep hlctl's columns, field rows and sections (from the
  templates in `homelab-cli/internal/cli/{docker,storage,system}/templates`),
  minus `DEVICE`. `--wide` on `list` adds columns hlctl only showed in `get`
  (e.g. volumes: filesystem, pool status; backups: last run, next run, size;
  containers: memory percent). `restartCount` is only on `containers get`:
  DSM's container list does not report it, so list items omit the field.
- `info` is a key/value table (one NAS). `utilization` is a key/value block
  for CPU, memory and swap, followed by `NETWORK` (per interface rx/tx per
  second, `total` excluded as in homelab-api) and `DISKS` (read/write ops per
  second) sections.
- **Colour**, on a terminal: status cells green (normal/running/success),
  red (crashed/failing/critical/dead/failed), yellow (degraded/warning/
  suspended), dim (stopped/idle); volume usage and disk temperature graded by
  the `status` thresholds below.
- **JSON** (`-o json`) prints the spec model without `device` and `uri`,
  pretty-printed with a trailing newline. Lists keep their `{"items": [...]}`
  wrapper. `info` and `utilization` print a single object, not a list.
- **Write commands** print `Started <name>`, `Stopped <name>`,
  `Restarted <name>` (hlctl's ActionCmd wording) and nothing under `-o json`.
- **Not found** messages: `Container not found: <name>`,
  `Image not found: <id>`, `Network not found: <name>`,
  `Volume not found: <id>`, `Backup task not found: <id>`,
  `Update not found: <name>`. Exit code 1.

### `docker containers list --status`

Filters on the mapped container status (`running`, `stopped`, `paused`,
`restarting`, `dead`), using homelab-api's `mapStatus` precedence: dead,
restarting, paused, running, otherwise stopped.

## Configuration

Path: `--config PATH` > `SYNOCTL_CONFIG` >
`$XDG_CONFIG_HOME/synoctl/config.toml` > `~/.config/synoctl/config.toml`.

```toml
host = "192.168.1.10:5001"     # required; host[:port]; always https
username = "synoctl"           # required; a dedicated DSM account is recommended
password = "…"                 # required
# insecure_tls = true          # DSM ships a self-signed certificate
# timezone = "Europe/Warsaw"   # IANA zone for backup times; default: local zone
# auth_version = 6             # SYNO.API.Auth login version; 3 for old DSM
# cache_ttl = 15               # seconds to reuse DSM responses; 0 disables

[updates]
# check_interval = "1h"        # how long release lookups are reused
[[updates.sources]]
image = "vaultwarden/server"   # image without tag
source = "dani-garcia/vaultwarden"
# type = "codeberg"            # default: github
```

- Env overrides: `SYNO_HOST`, `SYNO_USERNAME`, `SYNO_PASSWORD`,
  `SYNO_INSECURE_TLS`, `SYNO_TIMEZONE`, `SYNO_AUTH_VERSION`, `SYNO_CACHE_TTL`.
  `GITHUB_TOKEN`, when set, is sent as a bearer token to the GitHub API only.
  `updates.sources` is file-only.
- A missing file is fine when env supplies the required keys.
- Validation messages name the key and its env var, e.g.
  `config: host is required (set host in <path> or SYNO_HOST)`. An invalid
  `timezone` or `check_interval` is a config error.
- A group/other-readable file produces a warning on stderr; the command
  continues.

### `config init` / `config check`

Adapted from unifictl's `setup` module.

- `config init` prompts for host, username, password (hidden input) and
  whether to accept a self-signed certificate; flags (`--host`, `--username`,
  `--password`, `--insecure-tls`, `--force`, `--no-check`) and
  `SYNO_PASSWORD` serve scripts. It writes the file with mode 0600 and then
  runs the check.
- `config check` walks: config parse and validation, file permissions, DNS,
  TCP, TLS (suggesting `insecure_tls` on a certificate error),
  `SYNO.API.Info` reachability, login, then Container Manager and Hyper Backup
  availability (reported as "not installed", which is not a failure). It names
  the failing step and what to change, and exits non-zero on failure.

## DSM client

Ported from `homelab-api/internal/adapters/synology.go`.

- Blocking `reqwest` with rustls, 30 s timeout; `insecure_tls` →
  `danger_accept_invalid_certs(true)`.
- Every request is `GET https://{host}/webapi/{path}?{params}`; responses use
  the `{success, data, error: {code}}` envelope. `success: false` becomes
  `DsmError { api, code }`.
- **Login**: `SYNO.API.Info` `query` for `SYNO.API.Auth` (on `query.cgi`)
  gives the auth path; then `SYNO.API.Auth` `login` with `account`, `passwd`,
  `format=sid` and the configured `auth_version`. A failed login reports the
  DSM code (`400` → wrong account or password, `403`/`404` → 2-step
  verification required, which synoctl does not support).
- **Session cache**: the SID is stored in `$XDG_CACHE_HOME/synoctl/session.json`
  (or `~/.cache/synoctl/session.json`), mode 0600, together with the host and
  username it belongs to. A stored SID for a different host or username is
  ignored. Calls use the stored SID; on DSM codes 106, 107 or 119 the stored
  SID is discarded, synoctl logs in again, stores the new SID and retries the
  call once. No logout.
- **Not installed**: DSM code 102 ("API does not exist") on a `SYNO.Docker.*`
  call becomes `Container Manager is not installed on <host>`; on a
  `SYNO.Backup.*` call, `Hyper Backup is not installed on <host>`. The code is
  confirmed against a real NAS during implementation; if DSM returns a
  different code, the mapping follows the captured response.
- **Not found**: code 117 on container calls becomes `Container not found`.
- **Response cache**: unifictl's `cache.rs`, keyed by API, method, version and
  parameters (never the SID), fresh for `cache_ttl`. Only read calls are
  cached. `start`/`stop`/`restart` are never cached and remove the cached
  container list, container detail and container resource entries.
- **Calls** (`trait DsmApi`, one method each), with homelab-api's parameters:

  | Method | API | DSM method / version |
  |---|---|---|
  | `system_info` | `SYNO.Core.System` | `info` / 1 |
  | `system_utilization` | `SYNO.Core.System.Utilization` | `get` / 1 |
  | `storage` | `SYNO.Storage.CGI.Storage` | `load_info` / 1 |
  | `containers` | `SYNO.Docker.Container` | `list` / 1 (`limit=0`, `offset=0`) |
  | `container` | `SYNO.Docker.Container` | `get` / 1 (`name`) |
  | `container_resources` | `SYNO.Docker.Container.Resource` | `get` / 1 |
  | `start` / `stop` / `restart` | `SYNO.Docker.Container` | same name / 1 (`name`) |
  | `images` | `SYNO.Docker.Image` | `list` / 1 (`limit=-1`, `offset=0`, `show_dsm=false`) |
  | `networks` | `SYNO.Docker.Network` | `list` / 1 |
  | `backup_tasks` | `SYNO.Backup.Task` | `list` / 1 |
  | `backup_task` | `SYNO.Backup.Task` | `get` / 1 (`additional=["repository","schedule"]`) |
  | `backup_status` | `SYNO.Backup.Task` | `status` / 1 (`blOnline=false`, homelab-api's `additional` list) |
  | `backup_target` | `SYNO.Backup.Target` | `get` / 1 (homelab-api's `additional` list) |

- **Raw types** (`dsm/types.rs`) are ported from the Go structs. Fields added
  for `status` (system temperature warning, disk temperature, backup end
  time) are taken from captured responses. Following homelab-api's adapter
  rules, no response structure or fixture is invented: new fields come from
  responses captured with the `homelab-api/scripts/dsm-*.sh` approach, kept
  outside the repo, and sanitized before they become fixtures.

## Services

Each Go service function is ported into `service/<resource>.rs` (Docker
resources into `service/docker/<resource>.rs`), minus the
multi-backend loop, availability monitor and composite-ID parsing:

- **info**: model, firmware, RAM (MB), uptime parsed from `H:M:S` (0 when
  unparseable).
- **utilization**: CPU user/system/total (user + system + other), memory and
  swap in bytes (DSM reports KB), used percent, per-interface network (minus
  `total`), per-disk I/O, `sampledAt` = now.
- **volumes**: sizes parsed from strings; volume status and disk status maps
  as in homelab-api (unknown volume status → degraded, unknown disk status →
  warning, without the log line); `get` joins volume → pool (`pool_path`) →
  disks.
- **backups**: list = task list + per-task status for `lastResult`; `get` adds
  task detail (source folders), status (last success and next run, parsed as
  `YYYY/MM/DD HH:MM` in the configured time zone) and target (used size).
  A failed status/detail/target lookup leaves its fields empty, as in Go,
  with a warning on stderr. State, result and type mappings are copied
  (`done` + non-zero error code → `warning`; `image:*` → `hyperBackup`;
  `*glacier*` → `glacierBackup`).
- **containers**: list joins resources by name; `get` maps profile + details
  (restart policy, port, network and volume bindings, env, entrypoint, cmd,
  labels, `finishedAt` only when not running).
- **images**: short ID, tags (empty when null); `get` matches
  `sha256:<id>` prefix, created as UTC from Unix seconds.
- **networks**: connected container count; `get` adds driver, subnet,
  gateway, IP range (omitted when empty) and container names.
- **updates** and **status**: below.

## `docker updates`

Ported from `updates_service.go` and `github.go`.

- The container list is always read (through the response cache). Containers
  whose tag is empty, `latest` or a `sha256:` digest are not tracked.
- Source resolution: `updates.sources` by image (without tag) first, then
  `ghcr.io/<owner>/<repo>[/…]` → GitHub `<owner>/<repo>`. GitHub sources use
  `https://api.github.com`, Codeberg sources `https://codeberg.org/api/v1`;
  both read `/repos/{owner}/{repo}/releases/latest`.
- Status: `upToDate` when the latest tag equals the running tag ignoring a
  leading `v`; `updateAvailable` otherwise; `stale` when a source is
  configured but no release is known; `unknown` when no source is configured.
  An image without a source prints one warning per run on stderr, naming the
  `updates.sources` setting.
- Releases are fetched in parallel (scoped threads, a few at a time), 10 s
  timeout each, and cached in `$XDG_CACHE_HOME/synoctl/releases.json` (0600)
  with a fetch time. The cache is reused for `check_interval` (default 1h).
  A failed fetch keeps the previously cached release for that repo.
- `list [--status upToDate|updateAvailable|stale|unknown]`; `get <name>` adds
  image, source URL, release URL and published date; `check` ignores the
  release cache and prints the full list.

## `status`

One screen answering "is anything wrong on the NAS?", modelled on
`unifictl status`: a row per area with a verdict (`ok`, `info`, `warn`,
`error`, `unknown`), and the culprits named on indented lines below.

```
SYSTEM      ok     DS920+  DSM 7.2.2-72806  up 41d 3h  42°C
VOLUMES     warn   2 volumes
                   volume_2 87% used (8.1 TB of 9.3 TB)
DISKS       ok     4 disks, hottest 39°C
CONTAINERS  warn   14 of 15 running
                   stopped: watchtower
BACKUPS     error  3 tasks
                   failed: Backup to B2 (last success Oct 9 02:00)
UPDATES     info   2 containers behind
                   immich-server v1.118.0 → v1.119.1, grafana 11.2.0 → 11.3.0
```

(Illustrative values.)

| Row | Source | warn | error |
|---|---|---|---|
| SYSTEM | system info | DSM reports a system temperature warning | — |
| VOLUMES | storage | degraded or repairing, or ≥ 80% used | crashed, or ≥ 95% used |
| DISKS | storage | disk status `warning`, or ≥ 50 °C | `failing` or `critical` |
| CONTAINERS | container list | any container not running | any container `dead` |
| BACKUPS | tasks + status | last result `warning` or `skipped`, or task suspended | last result `failed` |
| UPDATES | updates service | — (`info` when any update is available) | — |

- The command exits 1 when any row is `error`.
- A row whose data cannot be read shows `unknown` with the reason and does not
  fail the command. CONTAINERS and UPDATES are omitted when Container Manager
  is not installed; BACKUPS when Hyper Backup is not installed.
- UPDATES uses the release cache and fetches only when it is older than
  `check_interval`; a failed fetch with no cached release makes the row
  `unknown`.
- `-o json` prints the rows (area, verdict, summary, problems) only.

## Project

Repository: `synoctl`, its own git repository at
`~/Projects/homelab/synoctl`. Binary: `synoctl`.

Stack and versions follow unifictl's `Cargo.toml`: Rust 1.99.0 (2024 edition)
via mise, `clap` + `clap_complete` (dynamic completion), blocking `reqwest`
with rustls, `serde`/`serde_json`, `toml`, `jiff`, `tabwriter`, `thiserror`,
`rpassword`; `httpmock` and `tempfile` for tests.

```
synoctl/
  Cargo.toml, Cargo.lock, mise.toml, mise.lock, rust-toolchain.toml
  dist-workspace.toml, release-plz.toml, .commitlintrc.json, .github/
  src/
    main.rs, lib.rs, app.rs   parse → config → client → dispatch → render → exit code
    cli.rs                    clap tree (flat + `docker` group), global flags
    config.rs, error.rs
    cache.rs, private_fs.rs, timespec.rs, completion.rs   from unifictl
    dsm/
      mod.rs                  trait DsmApi
      client.rs               HTTP, envelope, login, re-auth, error mapping
      session.rs              session.json
      types.rs                raw DSM structs
      fixtures.rs             fixture-backed DsmApi (feature "fixtures")
    releases.rs               GitHub/Codeberg latest release + releases.json
    model.rs                  output types (spec schemas minus device/uri)
    service/
      info.rs, utilization.rs, volumes.rs, backups.rs, status.rs
      docker/                 containers.rs, images.rs, networks.rs, updates.rs
    render/
      format.rs, style.rs     from unifictl
      info.rs, utilization.rs, volumes.rs, backups.rs, status.rs
      docker/                 containers.rs, images.rs, networks.rs, updates.rs
    setup/                    config init / check
  tests/
    fixtures/                 sanitized DSM JSON
    cli.rs, completion.rs     binary run with --fixtures
```

Dependency direction: `app → service → dsm::DsmApi`; `service → model`;
`render → model`. Services never see HTTP, credentials or the SID.

Everything Docker-related lives under a `docker` directory, mirroring the
`synoctl docker …` command group: `service/docker/` and `render/docker/` hold
containers, images, networks and updates, and in `cli.rs` the `docker`
subcommand enum owns its four resources. The flat commands (info,
utilization, volumes, backups, status) sit directly in `service/` and
`render/`. `status` reads containers and updates through
`service::docker`; nothing outside `docker/` maps Docker data itself.

## Testing

- **Services**: unit tests ported from homelab-api's `*_service_test.go`,
  run against the fixture `DsmApi`. Starting fixtures are homelab-api's
  sanitized `testdata` (system info/utilization/storage, backup
  tasks/detail/status/target, container list/detail/resources, image list,
  network list).
- **Renderers**: expected table output per command, with and without colour.
- **CLI**: `tests/cli.rs` runs the binary with `--fixtures` — exit codes,
  `-o json`, `-o ids`, not-found messages, `status` exit 1 on error.
- **Client**: `httpmock` tests for the envelope, login → SID stored → reuse,
  re-auth and single retry on 106/107/119, 117 → not found, 102 → not
  installed, write commands invalidating cached container entries.
- **Releases**: `httpmock` tests for fetch, cache reuse within
  `check_interval`, and a failed fetch keeping the previous release.
- `mise run test` (`cargo test --features fixtures`) and `mise run lint`
  (clippy `-D warnings` + `cargo fmt --check`), as in unifictl.

## Release

Same pipeline as unifictl: conventional commits checked by commitlint;
release-plz keeps a release PR with the version bump and `CHANGELOG.md`;
merging it tags `vX.Y.Z`; `dist` builds archives for macOS (arm64, x86_64)
and Linux (arm64, x86_64) on native runners; install with
`mise use -g github:bwilczynski/synoctl`. The README follows unifictl's
structure (Install, Configure, Use, Status, Shell completions, Relationship
to hlctl, Develop, Release). Creating the GitHub repository and pushing are
left to the author.
