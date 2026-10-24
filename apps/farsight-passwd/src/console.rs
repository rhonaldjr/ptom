//! Reading passwords and answers from the user.

use std::io::{self, BufRead, IsTerminal, Read, Write};

use anyhow::{Context as _, bail};
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use crate::cli::PasswordInput;

/// Longest password line accepted from stdin, in bytes.
const MAX_LINE: u64 = 4096;

/// How commands talk to the user. Tests substitute a scripted version.
pub trait Console {
    /// Reads a new password. On a terminal, asks twice and checks they match.
    fn new_password(&mut self, prompt: &str) -> anyhow::Result<SecretString>;

    /// Asks a yes/no question. `None` if nobody can answer (not a terminal).
    fn confirm(&mut self, question: &str) -> anyhow::Result<Option<bool>>;
}

/// The real console: the controlling terminal, or stdin with `--stdin`.
pub struct SystemConsole {
    input: PasswordInput,
}

impl SystemConsole {
    pub fn new(input: PasswordInput) -> Self {
        Self { input }
    }
}

impl Console for SystemConsole {
    fn new_password(&mut self, prompt: &str) -> anyhow::Result<SecretString> {
        match self.input {
            PasswordInput::Stdin => read_password_line(&mut io::stdin().lock()),
            PasswordInput::Terminal => read_password_twice(prompt),
        }
    }

    fn confirm(&mut self, question: &str) -> anyhow::Result<Option<bool>> {
        let stdin = io::stdin();
        if self.input == PasswordInput::Stdin || !stdin.is_terminal() {
            return Ok(None);
        }
        eprint!("{question} [y/N] ");
        io::stderr().flush().ok();
        let mut answer = String::new();
        stdin.lock().read_line(&mut answer)?;
        Ok(Some(matches!(
            answer.trim().to_lowercase().as_str(),
            "y" | "yes"
        )))
    }
}

fn read_password_twice(prompt: &str) -> anyhow::Result<SecretString> {
    let first = prompt_hidden(prompt)?;
    let second = prompt_hidden("Type it again to confirm: ")?;
    if first.expose_secret() != second.expose_secret() {
        bail!("the two passwords did not match. Nothing was changed.");
    }
    Ok(first)
}

fn prompt_hidden(prompt: &str) -> anyhow::Result<SecretString> {
    // rpassword reads from the controlling terminal with echo turned off.
    let text = Zeroizing::new(
        rpassword::prompt_password(prompt)
            .context("could not read a password from the terminal (try --stdin)")?,
    );
    Ok(SecretString::from(text.as_str()))
}

/// Reads one line as a password, without the line ending.
pub fn read_password_line(input: &mut dyn BufRead) -> anyhow::Result<SecretString> {
    let mut bytes = Zeroizing::new(Vec::new());
    input
        .take(MAX_LINE + 1)
        .read_until(b'\n', &mut bytes)
        .context("could not read the password from standard input")?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    } else if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_LINE {
        bail!("the password on standard input is too long");
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        bail!("the password on standard input is not valid UTF-8 text");
    };
    Ok(SecretString::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(input: &[u8]) -> anyhow::Result<String> {
        read_password_line(&mut &input[..]).map(|s| s.expose_secret().to_owned())
    }

    #[test]
    fn strips_line_endings_only() {
        assert_eq!(line(b"pw\n").unwrap(), "pw");
        assert_eq!(line(b"pw\r\n").unwrap(), "pw");
        assert_eq!(line(b" pw \nignored").unwrap(), " pw ");
        assert_eq!(line(b"pw").unwrap(), "pw");
    }

    #[test]
    fn rejects_overlong_and_non_utf8_input() {
        assert!(line(&[b'x'; 5000]).is_err());
        assert!(line(b"\xff\xfe\n").is_err());
    }
}
