use thiserror::Error;

/// Every failure synoctl reports. `Display` is the user-facing message;
/// `app::main` prefixes it with `Error: `.
#[derive(Debug, Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),
    #[error("{0}")]
    NotFound(String),
    /// A DSM package (Container Manager, Hyper Backup) is missing.
    #[error("{0}")]
    NotInstalled(String),
    #[error("{0}")]
    Dsm(String),
    /// The NAS refused the credentials; retrying will not help.
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
