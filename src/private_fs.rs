//! Owner-only files and directories, for anything holding secrets or network
//! inventory: the config file and the response cache.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Creates `dir` and missing parents, `0700` on Unix.
#[cfg(unix)]
pub(crate) fn create_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
pub(crate) fn create_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)
}

/// Writes `body` to `path`, creating it `0600` on Unix.
fn write(path: &Path, body: &str) -> std::io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(body.as_bytes())
}

/// Writes `body` to a fresh temporary sibling and renames it over `path`, so
/// readers never see a partial file and the result is owner-only even when
/// it replaces a world-readable one.
pub(crate) fn replace(path: &Path, body: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let _ = fs::remove_file(&tmp);
    let result = write(&tmp, body).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
