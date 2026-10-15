use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::engine::ArgValueCandidates;

use crate::completion;

use crate::model::docker::{ContainerStatus, UpdateStatus};

#[derive(Debug, Parser)]
#[command(
    name = "synoctl",
    version,
    about = "Command-line client for Synology NAS (DSM)"
)]
pub struct Cli {
    /// Output format
    #[arg(short, long, global = true, value_enum, default_value_t = OutputFormat::Table)]
    pub output: OutputFormat,

    /// When to colour status cells in tables
    #[arg(long, global = true, value_enum, value_name = "WHEN", default_value_t = ColorMode::Auto)]
    pub color: ColorMode,

    /// Ignore cached NAS responses and fetch fresh ones
    #[arg(long, global = true)]
    pub refresh: bool,

    /// Config file (default: ~/.config/synoctl/config.toml)
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Read NAS data from fixture files instead of a NAS (testing only)
    #[cfg(feature = "fixtures")]
    #[arg(long, global = true, hide = true, value_name = "DIR")]
    pub fixtures: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    /// One ID per line (list commands)
    Ids,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorMode {
    /// When stdout is a terminal and NO_COLOR is unset or empty
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// One screen of NAS health: system, volumes, disks, containers, backups, updates
    Status,
    /// Model, DSM version, memory and uptime
    Info,
    /// Live CPU, memory, network and disk activity
    Utilization,
    /// Storage volumes
    Volumes {
        #[command(subcommand)]
        command: VolumesCommand,
    },
    /// Hyper Backup tasks
    Backups {
        #[command(subcommand)]
        command: BackupsCommand,
    },
    /// Container Manager: containers, images, networks and image updates
    Docker {
        #[command(subcommand)]
        command: DockerCommand,
    },
    /// Create the config file, or check it and the connection to the NAS
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Print a shell completion script
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Powershell,
    Elvish,
}

#[derive(Debug, Subcommand)]
pub enum VolumesCommand {
    /// List volumes
    List {
        /// Show additional columns (usage, filesystem)
        #[arg(long)]
        wide: bool,
    },
    /// Show a volume and its disks
    Get {
        #[arg(add = ArgValueCandidates::new(completion::volumes))]
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum BackupsCommand {
    /// List backup tasks and their last result
    List {
        /// Show additional columns (last run, next run, size)
        #[arg(long)]
        wide: bool,
    },
    /// Show a backup task's runs, size and folders
    Get {
        #[arg(add = ArgValueCandidates::new(completion::backups))]
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum DockerCommand {
    /// Containers
    Containers {
        #[command(subcommand)]
        command: ContainersCommand,
    },
    /// Images
    Images {
        #[command(subcommand)]
        command: ImagesCommand,
    },
    /// Docker networks
    Networks {
        #[command(subcommand)]
        command: NetworksCommand,
    },
    /// Running image versions against the latest upstream releases
    Updates {
        #[command(subcommand)]
        command: UpdatesCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ContainersCommand {
    /// List containers with their CPU and memory use
    List {
        /// Only containers in this state
        #[arg(long, value_enum)]
        status: Option<ContainerStatusArg>,
        /// Show additional columns (restarts, memory share)
        #[arg(long)]
        wide: bool,
    },
    /// Show a container's configuration and state
    Get {
        #[arg(add = ArgValueCandidates::new(completion::containers))]
        name: String,
    },
    /// Start a container
    Start {
        #[arg(add = ArgValueCandidates::new(completion::containers))]
        name: String,
    },
    /// Stop a container
    Stop {
        #[arg(add = ArgValueCandidates::new(completion::containers))]
        name: String,
    },
    /// Restart a container
    Restart {
        #[arg(add = ArgValueCandidates::new(completion::containers))]
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ImagesCommand {
    /// List images
    List,
    /// Show an image
    Get {
        #[arg(add = ArgValueCandidates::new(completion::images))]
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum NetworksCommand {
    /// List Docker networks
    List,
    /// Show a network and its containers
    Get {
        #[arg(add = ArgValueCandidates::new(completion::networks))]
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum UpdatesCommand {
    /// List tracked containers and whether a newer release exists
    List {
        /// Only updates in this state
        #[arg(long, value_enum)]
        status: Option<UpdateStatusArg>,
    },
    /// Show a tracked container's source and latest release
    Get {
        #[arg(add = ArgValueCandidates::new(completion::updates))]
        name: String,
    },
    /// Look up releases now, ignoring the cache, and list the result
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ContainerStatusArg {
    Running,
    Stopped,
    Paused,
    Restarting,
    Dead,
}

impl From<ContainerStatusArg> for ContainerStatus {
    fn from(a: ContainerStatusArg) -> Self {
        match a {
            ContainerStatusArg::Running => Self::Running,
            ContainerStatusArg::Stopped => Self::Stopped,
            ContainerStatusArg::Paused => Self::Paused,
            ContainerStatusArg::Restarting => Self::Restarting,
            ContainerStatusArg::Dead => Self::Dead,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UpdateStatusArg {
    #[value(name = "upToDate")]
    UpToDate,
    #[value(name = "updateAvailable")]
    UpdateAvailable,
    Stale,
    Unknown,
}

impl From<UpdateStatusArg> for UpdateStatus {
    fn from(a: UpdateStatusArg) -> Self {
        match a {
            UpdateStatusArg::UpToDate => Self::UpToDate,
            UpdateStatusArg::UpdateAvailable => Self::UpdateAvailable,
            UpdateStatusArg::Stale => Self::Stale,
            UpdateStatusArg::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Create the config file, prompting for anything not given as a flag
    Init(InitArgs),
    /// Check the config and the connection to the NAS, step by step
    Check,
}

#[derive(Debug, Default, Args)]
pub struct InitArgs {
    /// NAS host or host:port (always reached over https; DSM uses 5001)
    #[arg(long)]
    pub host: Option<String>,
    /// DSM account; a dedicated one without 2-step verification
    #[arg(long)]
    pub username: Option<String>,
    /// Password for --username; defaults to $SYNO_PASSWORD
    #[arg(long, conflicts_with = "password_command")]
    pub password: Option<String>,
    /// Shell command printing the password (e.g. `rbw get synoctl`), stored
    /// instead of the password and run whenever synoctl logs in
    #[arg(long, value_name = "COMMAND")]
    pub password_command: Option<String>,
    /// Accept the NAS's self-signed certificate
    #[arg(long)]
    pub insecure_tls: bool,
    /// Overwrite an existing config file
    #[arg(long)]
    pub force: bool,
    /// Do not contact the NAS: skip the certificate probe and the final check
    #[arg(long)]
    pub no_check: bool,
}
