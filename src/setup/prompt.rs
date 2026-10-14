//! Line prompts on stdin/stderr for `config init`.

use std::io::{IsTerminal, Write};

use crate::error::{Error, Result};

/// Where `config init` asks its questions. Tests use a scripted one.
pub trait Prompter {
    /// False when stdin is not a terminal: never prompt, fail instead.
    fn interactive(&self) -> bool;
    /// Shows `prompt` and reads one line without its newline; EOF is an error.
    fn read_line(&mut self, prompt: &str) -> Result<String>;
    /// Like `read_line`, without echoing the input.
    fn read_secret(&mut self, prompt: &str) -> Result<String>;
    /// Progress for the user, on stderr.
    fn note(&mut self, msg: &str);

    /// `label [default]: `; an empty answer takes the default.
    fn line(&mut self, label: &str, default: Option<&str>) -> Result<String> {
        let prompt = match default {
            Some(d) => format!("{label} [{d}]: "),
            None => format!("{label}: "),
        };
        let answer = self.read_line(&prompt)?.trim().to_string();
        Ok(match default {
            Some(d) if answer.is_empty() => d.to_string(),
            _ => answer,
        })
    }

    /// `label [y/N]: `; anything starting with `y` is yes.
    fn confirm(&mut self, label: &str, default: bool) -> Result<bool> {
        let choices = if default { "Y/n" } else { "y/N" };
        let answer = self.read_line(&format!("{label} [{choices}]: "))?;
        Ok(match answer.trim().to_lowercase().as_str() {
            "" => default,
            a => a.starts_with('y'),
        })
    }
}

pub struct TerminalPrompter;

fn aborted() -> Error {
    Error::Other("config init: aborted, nothing written".into())
}

impl Prompter for TerminalPrompter {
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn read_line(&mut self, prompt: &str) -> Result<String> {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => Err(aborted()),
            Ok(_) => Ok(line.trim_end_matches(['\r', '\n']).to_string()),
            Err(e) => Err(Error::Other(format!("read input: {e}"))),
        }
    }

    fn read_secret(&mut self, prompt: &str) -> Result<String> {
        rpassword::prompt_password(prompt).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => aborted(),
            _ => Error::Other(format!("read input: {e}")),
        })
    }

    fn note(&mut self, msg: &str) {
        eprintln!("{msg}");
    }
}
