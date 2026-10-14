use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::cache::Cache;
use crate::cli::{
    BackupsCommand, Cli, ColorMode, Command, ConfigCommand, ContainersCommand, DockerCommand,
    ImagesCommand, NetworksCommand, OutputFormat, UpdatesCommand, VolumesCommand,
};
use crate::completion;
use crate::config;
use crate::dsm::client::DsmClient;
use crate::dsm::session::SessionStore;
use crate::error::{Error, Result};
use crate::model::Ids;
use crate::model::docker::Action;
use crate::releases::HttpReleases;
use crate::render;
use crate::service::{Service, Warned};
use crate::setup;

/// Entry point used by `main.rs`; returns the process exit code.
pub fn main() -> i32 {
    completion::handle_request();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };
    match run(cli) {
        Ok(out) => match write_stdout(&out.stdout) {
            0 => out.code,
            code => code,
        },
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    }
}

/// A reader that went away (`| head`, `| grep -q`) is not an error.
fn write_stdout(out: &str) -> i32 {
    use std::io::{ErrorKind, Write};
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(out.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => 0,
        Err(e) if e.kind() == ErrorKind::BrokenPipe => 0,
        Err(e) => {
            eprintln!("Error: write output: {e}");
            1
        }
    }
}

/// What a command prints on stdout and the exit code that follows it.
pub struct Outcome {
    pub stdout: String,
    pub code: i32,
}

impl Outcome {
    fn ok(stdout: String) -> Self {
        Self { stdout, code: 0 }
    }
}

/// Executes a parsed command.
pub fn run(cli: Cli) -> Result<Outcome> {
    let env = |k: &str| std::env::var(k).ok();
    if let Command::Completions { shell } = &cli.command {
        return completion::script(*shell).map(Outcome::ok);
    }
    if cli.output == OutputFormat::Ids && !lists_ids(&cli.command) {
        return Err(Error::Other(
            "-o ids is only supported by list commands".into(),
        ));
    }
    let color = color_enabled(cli.color, cli.output, std::io::stdout().is_terminal(), &env);
    if let Command::Config { command } = &cli.command {
        return run_config(command, &cli, &env, color);
    }
    let svc = build_service(&cli)?;
    let ctx = Ctx {
        out: cli.output,
        color,
        tz: TimeZone::system(),
        now: now(&env),
    };
    if let Command::Status = &cli.command {
        let status = svc.status(ctx.now);
        return Ok(Outcome {
            stdout: emit(ctx.out, &status, |s| render::status::table(s, color))?,
            code: i32::from(status.has_error()),
        });
    }
    execute(&svc, &cli.command, &ctx).map(Outcome::ok)
}

fn run_config(
    cmd: &ConfigCommand,
    cli: &Cli,
    env: &dyn Fn(&str) -> Option<String>,
    color: bool,
) -> Result<Outcome> {
    let (path, explicit) = config_location(cli.config.as_deref(), env);
    if let ConfigCommand::Init(args) = cmd {
        setup::init::run(args, &path, env, &mut setup::prompt::TerminalPrompter)?;
        if args.no_check {
            return Ok(Outcome::ok(String::new()));
        }
    }
    let report = setup::check::run(&path, explicit, env);
    let code = if report.ok { 0 } else { 1 };
    Ok(Outcome {
        stdout: emit(cli.output, &report, |r| render::check::table(r, color))?,
        code,
    })
}

/// How to print, and the clock.
struct Ctx {
    out: OutputFormat,
    color: bool,
    /// The viewer's zone, for times in tables.
    tz: TimeZone,
    now: Timestamp,
}

/// Tables only; under `auto`, only on a terminal and with `NO_COLOR` unset or
/// empty (no-color.org).
fn color_enabled(
    mode: ColorMode,
    out: OutputFormat,
    is_tty: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> bool {
    if out != OutputFormat::Table {
        return false;
    }
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => is_tty && env("NO_COLOR").is_none_or(|v| v.is_empty()),
    }
}

/// The config file in effect, and whether the user named it.
pub(crate) fn config_location(
    config_flag: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> (PathBuf, bool) {
    let path = config::resolve_path(config_flag, env);
    let explicit = config_flag.is_some() || env("SYNOCTL_CONFIG").is_some_and(|v| !v.is_empty());
    (path, explicit)
}

/// The current time. Fixture builds read `SYNOCTL_NOW` (RFC 3339) so tests
/// get a fixed clock.
fn now(env: &dyn Fn(&str) -> Option<String>) -> Timestamp {
    #[cfg(feature = "fixtures")]
    if let Some(t) = env("SYNOCTL_NOW").and_then(|v| v.parse().ok()) {
        return t;
    }
    #[cfg(not(feature = "fixtures"))]
    let _ = env;
    Timestamp::now()
}

fn build_service(cli: &Cli) -> Result<Service> {
    #[cfg(feature = "fixtures")]
    if let Some(dir) = &cli.fixtures {
        return fixture_service(dir);
    }
    let (svc, warnings) = nas_service(cli.config.as_deref(), cli.refresh, None)?;
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    Ok(svc)
}

#[cfg(feature = "fixtures")]
pub(crate) fn fixture_service(dir: &Path) -> Result<Service> {
    let api = crate::dsm::fixtures::FixtureApi::load(dir)?;
    let releases = crate::releases::FixtureReleases::load(dir)?;
    Ok(Service::new(Box::new(api), TimeZone::UTC).with_updates(
        Box::new(releases),
        Vec::new(),
        "the config file".into(),
    ))
}

/// A service for the configured NAS, and the config warnings for the caller
/// to show or not. `timeout` overrides the default request timeout.
pub(crate) fn nas_service(
    config_flag: Option<&Path>,
    refresh: bool,
    timeout: Option<Duration>,
) -> Result<(Service, Vec<String>)> {
    let env = |k: &str| std::env::var(k).ok();
    let (path, explicit) = config_location(config_flag, &env);
    let loaded = config::load(&path, explicit, &env)?;
    let cfg = &loaded.config;
    let cache_dir = config::resolve_cache_dir(&env);
    let mut client = DsmClient::new(cfg)?.with_session_store(SessionStore::new(&cache_dir));
    if let Some(timeout) = timeout {
        client = client.with_timeout(timeout);
    }
    if !cfg.cache_ttl.is_zero() {
        client = client.with_cache(Cache::new(cache_dir.clone(), cfg.cache_ttl, !refresh));
    }
    let releases = HttpReleases::new(
        Some(&cache_dir),
        cfg.updates.check_interval,
        env("GITHUB_TOKEN"),
    )?;
    let svc = Service::new(Box::new(client), cfg.timezone.clone()).with_updates(
        Box::new(releases),
        cfg.updates.sources.clone(),
        path.display().to_string(),
    );
    Ok((svc, loaded.warnings))
}

/// Prints `w`'s warnings on stderr and hands back its data.
fn warned<T>(w: Warned<T>) -> T {
    for msg in &w.warnings {
        eprintln!("warning: {msg}");
    }
    w.data
}

fn execute(svc: &Service, cmd: &Command, ctx: &Ctx) -> Result<String> {
    let (out, color, tz) = (ctx.out, ctx.color, &ctx.tz);
    match cmd {
        Command::Info => emit(out, &svc.info()?, render::info::table),
        Command::Utilization => emit(out, &svc.utilization(ctx.now)?, render::utilization::table),
        Command::Volumes { command } => match command {
            VolumesCommand::List { wide } => emit_list(out, &svc.list_volumes()?, |l| {
                render::volumes::list(l, *wide, color)
            }),
            VolumesCommand::Get { id } => emit(out, &svc.get_volume(id)?, |d| {
                render::volumes::get(d, color)
            }),
        },
        Command::Backups { command } => match command {
            BackupsCommand::List { wide: true } if out == OutputFormat::Table => {
                let l = warned(svc.list_backups_wide()?);
                Ok(render::backups::list_wide(&l, tz, color))
            }
            BackupsCommand::List { .. } => emit_list(out, &warned(svc.list_backups()?), |l| {
                render::backups::list(l, color)
            }),
            BackupsCommand::Get { id } => emit(out, &warned(svc.get_backup(id)?), |d| {
                render::backups::get(d, tz, color)
            }),
        },
        Command::Docker { command } => docker(svc, command, ctx),
        Command::Status | Command::Config { .. } | Command::Completions { .. } => {
            unreachable!("status, config and completions are handled by run")
        }
    }
}

fn docker(svc: &Service, cmd: &DockerCommand, ctx: &Ctx) -> Result<String> {
    let (out, color, tz) = (ctx.out, ctx.color, &ctx.tz);
    match cmd {
        DockerCommand::Containers { command } => match command {
            ContainersCommand::List { status, wide } => {
                let l = svc.list_containers(status.map(Into::into))?;
                emit_list(out, &l, |l| {
                    render::docker::containers::list(l, *wide, color)
                })
            }
            ContainersCommand::Get { name } => emit(out, &svc.get_container(name)?, |d| {
                render::docker::containers::get(d, tz, color)
            }),
            ContainersCommand::Start { name } => act(svc, name, Action::Start, out),
            ContainersCommand::Stop { name } => act(svc, name, Action::Stop, out),
            ContainersCommand::Restart { name } => act(svc, name, Action::Restart, out),
        },
        DockerCommand::Images { command } => match command {
            ImagesCommand::List => {
                emit_list(out, &svc.list_images()?, render::docker::images::list)
            }
            ImagesCommand::Get { id } => emit(out, &svc.get_image(id)?, |d| {
                render::docker::images::get(d, tz)
            }),
        },
        DockerCommand::Networks { command } => match command {
            NetworksCommand::List => {
                emit_list(out, &svc.list_networks()?, render::docker::networks::list)
            }
            NetworksCommand::Get { name } => {
                emit(out, &svc.get_network(name)?, render::docker::networks::get)
            }
        },
        DockerCommand::Updates { command } => {
            let list = |status: Option<_>, force| -> Result<String> {
                let l = warned(svc.list_updates(status, force, ctx.now)?);
                emit_list(out, &l, |l| render::docker::updates::list(l, color))
            };
            match command {
                UpdatesCommand::List { status } => list(status.map(Into::into), false),
                UpdatesCommand::Check => list(None, true),
                UpdatesCommand::Get { name } => {
                    emit(out, &warned(svc.get_update(name, ctx.now)?), |d| {
                        render::docker::updates::get(d, tz, color)
                    })
                }
            }
        }
    }
}

/// start/stop/restart: a confirmation in tables, nothing in JSON.
fn act(svc: &Service, name: &str, action: Action, out: OutputFormat) -> Result<String> {
    svc.container_action(name, action)?;
    Ok(match out {
        OutputFormat::Table => format!("{} {name}\n", action.past()),
        _ => String::new(),
    })
}

fn emit<T: Serialize>(
    out: OutputFormat,
    value: &T,
    table: impl FnOnce(&T) -> String,
) -> Result<String> {
    match out {
        OutputFormat::Json => json(value),
        OutputFormat::Table => Ok(table(value)),
        OutputFormat::Ids => Err(Error::Other(
            "-o ids is only supported by list commands".into(),
        )),
    }
}

/// `emit` for lists, which can also print just their IDs.
fn emit_list<T: Serialize + Ids>(
    out: OutputFormat,
    value: &T,
    table: impl FnOnce(&T) -> String,
) -> Result<String> {
    if out == OutputFormat::Ids {
        return Ok(value.ids().iter().map(|id| format!("{id}\n")).collect());
    }
    emit(out, value, table)
}

/// Commands whose output is a list of items with IDs.
fn lists_ids(cmd: &Command) -> bool {
    match cmd {
        Command::Volumes { command } => matches!(command, VolumesCommand::List { .. }),
        Command::Backups { command } => matches!(command, BackupsCommand::List { .. }),
        Command::Docker { command } => match command {
            DockerCommand::Containers { command } => {
                matches!(command, ContainersCommand::List { .. })
            }
            DockerCommand::Images { command } => matches!(command, ImagesCommand::List),
            DockerCommand::Networks { command } => matches!(command, NetworksCommand::List),
            DockerCommand::Updates { command } => {
                matches!(command, UpdatesCommand::List { .. } | UpdatesCommand::Check)
            }
        },
        _ => false,
    }
}

fn json<T: Serialize>(value: &T) -> Result<String> {
    let mut s = serde_json::to_string_pretty(value)
        .map_err(|e| Error::Other(format!("encode JSON: {e}")))?;
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(no_color: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
        move |k| {
            (k == "NO_COLOR")
                .then_some(no_color)
                .flatten()
                .map(String::from)
        }
    }

    #[test]
    fn auto_colours_only_a_terminal() {
        let none = env_with(None);
        assert!(color_enabled(
            ColorMode::Auto,
            OutputFormat::Table,
            true,
            &none
        ));
        assert!(!color_enabled(
            ColorMode::Auto,
            OutputFormat::Table,
            false,
            &none
        ));
    }

    #[test]
    fn non_empty_no_color_disables_auto_but_empty_does_not() {
        let t = OutputFormat::Table;
        assert!(!color_enabled(
            ColorMode::Auto,
            t,
            true,
            &env_with(Some("1"))
        ));
        assert!(color_enabled(ColorMode::Auto, t, true, &env_with(Some(""))));
    }

    #[test]
    fn always_and_never_ignore_terminal_and_no_color() {
        let t = OutputFormat::Table;
        assert!(color_enabled(
            ColorMode::Always,
            t,
            false,
            &env_with(Some("1"))
        ));
        assert!(!color_enabled(ColorMode::Never, t, true, &env_with(None)));
    }

    #[test]
    fn json_is_never_coloured() {
        assert!(!color_enabled(
            ColorMode::Always,
            OutputFormat::Json,
            true,
            &env_with(None)
        ));
    }
}
