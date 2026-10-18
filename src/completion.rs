//! Shell completion through clap_complete's dynamic protocol: the script that
//! `completions <shell>` prints calls back into `COMPLETE=<shell> synoctl`
//! on every TAB. Subcommands and flags come from the clap definition, IDs from
//! the NAS (through the response cache, so repeated TABs are instant).

use std::path::PathBuf;
use std::time::Duration;

use clap::{CommandFactory, ValueEnum};
use clap_complete::engine::CompletionCandidate;
use clap_complete::env::{CompleteEnv, Shells};

use crate::app;
use crate::cli::{Cli, Shell};
use crate::error::{Error, Result};
use crate::service::docker::tracked_image;
use crate::service::{BackupFilter, ContainerFilter, Service, VolumeFilter};

/// Environment variable that turns a run into a completion request.
const VAR: &str = "COMPLETE";

/// A TAB waiting on an unreachable NAS should give up quickly.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Answers the shell and exits if this run is a completion request; returns
/// otherwise.
pub fn handle_request() {
    CompleteEnv::with_factory(Cli::command).var(VAR).complete();
}

/// The registration script for `shell`, the same one `COMPLETE=<shell>
/// synoctl` prints.
pub fn script(shell: Shell) -> Result<String> {
    let value = shell.to_possible_value().expect("no skipped variants");
    let shells = Shells::builtins();
    let completer = shells
        .completer(value.get_name())
        .expect("every Shell is a clap_complete builtin");
    let mut buf = Vec::new();
    completer
        .write_registration(VAR, "synoctl", "synoctl", &completer_path(), &mut buf)
        .map_err(|e| Error::Other(format!("write completion script: {e}")))?;
    String::from_utf8(buf).map_err(|e| Error::Other(format!("completion script: {e}")))
}

/// The binary the script calls on TAB: this one, as it was invoked. A
/// relative path (`./target/debug/synoctl`) is made absolute; a bare name is
/// left to `PATH`.
fn completer_path() -> String {
    let argv0 = PathBuf::from(
        std::env::args_os()
            .next()
            .unwrap_or_else(|| "synoctl".into()),
    );
    let path = if argv0.components().count() > 1 {
        let rel = argv0.strip_prefix(".").unwrap_or(&argv0);
        std::env::current_dir().map_or(argv0.clone(), |dir| dir.join(rel))
    } else {
        argv0
    };
    path.to_string_lossy().into_owned()
}

pub fn containers() -> Vec<CompletionCandidate> {
    ids(|svc| {
        let items = svc.list_containers(&ContainerFilter::default())?.items;
        Ok(items
            .into_iter()
            .map(|c| (c.id, c.status.as_str().to_string()))
            .collect())
    })
}

pub fn images() -> Vec<CompletionCandidate> {
    ids(|svc| {
        let items = svc.list_images()?.items;
        Ok(items
            .into_iter()
            .map(|i| {
                let help = format!("{}:{}", i.repository, i.tags.join(","));
                (i.id, help)
            })
            .collect())
    })
}

pub fn networks() -> Vec<CompletionCandidate> {
    ids(|svc| {
        let items = svc.list_networks()?.items;
        Ok(items
            .into_iter()
            .map(|n| (n.id, format!("{} containers", n.connected_containers)))
            .collect())
    })
}

pub fn volumes() -> Vec<CompletionCandidate> {
    ids(|svc| {
        let items = svc.list_volumes(&VolumeFilter::default())?.items;
        Ok(items
            .into_iter()
            .map(|v| (v.id, v.status.as_str().to_string()))
            .collect())
    })
}

pub fn backups() -> Vec<CompletionCandidate> {
    ids(|svc| {
        let items = svc.list_backups(&BackupFilter::default())?.data.items;
        Ok(items.into_iter().map(|t| (t.id, t.name)).collect())
    })
}

/// Tracked containers; release lookups are skipped, so a TAB never waits on
/// GitHub.
pub fn updates() -> Vec<CompletionCandidate> {
    ids(|svc| {
        let items = svc.list_containers(&ContainerFilter::default())?.items;
        Ok(items
            .into_iter()
            .filter(|c| tracked_image(&c.image).is_some())
            .map(|c| (c.id, c.image))
            .collect())
    })
}

/// `(id, help)` pairs from `list` as candidates. Any failure (no config,
/// NAS down) completes nothing: a TAB never prints an error.
fn ids(list: impl FnOnce(&Service) -> Result<Vec<(String, String)>>) -> Vec<CompletionCandidate> {
    let Ok(svc) = service() else {
        return Vec::new();
    };
    list(&svc)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, help)| CompletionCandidate::new(id).help(Some(help.into())))
        .collect()
}

/// The completer cannot see `--config` or `--fixtures` on the line being
/// completed, so it uses the environment and the default config path.
fn service() -> Result<Service> {
    // Testing only, like --fixtures.
    #[cfg(feature = "fixtures")]
    if let Some(dir) = std::env::var_os("SYNOCTL_FIXTURES") {
        return app::fixture_service(std::path::Path::new(&dir));
    }
    app::nas_service(None, false, Some(TIMEOUT)).map(|(svc, _warnings)| svc)
}
