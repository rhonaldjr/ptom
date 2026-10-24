//! TigerVNC-compatible server address syntax.
//!
//! - `host` → port 5900
//! - `host:N` → display N, port 5900 + N (N < 100), else port N
//! - `host::port` → that port
//! - `[v6addr]:N`, `[v6addr]::port` → IPv6

use thiserror::Error;

/// Base TCP port for display `:0`.
pub const BASE_PORT: u16 = 5900;

/// A parsed server address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    /// Hostname or IP address (without brackets).
    pub host: String,
    /// TCP port.
    pub port: u16,
}

/// Why an address could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AddressError {
    /// Nothing was given.
    #[error("no server address given")]
    Empty,
    /// The display or port is not a valid number.
    #[error("invalid display or port: {0}")]
    InvalidPort(String),
    /// An IPv6 address is missing its closing bracket.
    #[error("unterminated IPv6 address")]
    UnterminatedBracket,
}

impl ServerAddress {
    /// Parses the TigerVNC address syntax.
    ///
    /// # Errors
    /// [`AddressError`] if the address is empty or malformed.
    pub fn parse(input: &str) -> Result<Self, AddressError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(AddressError::Empty);
        }
        let (host, rest) = if let Some(after) = input.strip_prefix('[') {
            let end = after.find(']').ok_or(AddressError::UnterminatedBracket)?;
            (
                after.get(..end).unwrap_or_default(),
                after.get(end + 1..).unwrap_or_default(),
            )
        } else if input.matches(':').count() > 2 {
            // A bare IPv6 address without a display.
            (input, "")
        } else {
            match input.find(':') {
                Some(i) => (
                    input.get(..i).unwrap_or_default(),
                    input.get(i..).unwrap_or_default(),
                ),
                None => (input, ""),
            }
        };
        let host = if host.is_empty() { "localhost" } else { host };
        let port = parse_port(rest)?;
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }

    /// The address in TigerVNC syntax, using a display number when possible.
    #[must_use]
    pub fn display_name(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port.checked_sub(BASE_PORT) {
            Some(0) => host,
            Some(display) if display < 100 => format!("{host}:{display}"),
            _ => format!("{host}::{}", self.port),
        }
    }
}

fn parse_port(rest: &str) -> Result<u16, AddressError> {
    let invalid = || AddressError::InvalidPort(rest.to_owned());
    if rest.is_empty() {
        return Ok(BASE_PORT);
    }
    if let Some(port) = rest.strip_prefix("::") {
        return port.parse().map_err(|_| invalid());
    }
    let number: u16 = rest
        .strip_prefix(':')
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    if number < 100 {
        Ok(BASE_PORT + number)
    } else {
        Ok(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(host: &str, port: u16) -> ServerAddress {
        ServerAddress {
            host: host.into(),
            port,
        }
    }

    #[test]
    fn tigervnc_forms() {
        assert_eq!(ServerAddress::parse("pop-os"), Ok(addr("pop-os", 5900)));
        assert_eq!(ServerAddress::parse("pop-os:1"), Ok(addr("pop-os", 5901)));
        assert_eq!(
            ServerAddress::parse("pop-os:5905"),
            Ok(addr("pop-os", 5905))
        );
        assert_eq!(
            ServerAddress::parse("pop-os::2222"),
            Ok(addr("pop-os", 2222))
        );
        assert_eq!(ServerAddress::parse(":3"), Ok(addr("localhost", 5903)));
        assert_eq!(ServerAddress::parse("[::1]:1"), Ok(addr("::1", 5901)));
        assert_eq!(
            ServerAddress::parse("[fe80::1]::6000"),
            Ok(addr("fe80::1", 6000))
        );
        assert_eq!(
            ServerAddress::parse("fe80::1:2:3"),
            Ok(addr("fe80::1:2:3", 5900))
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(ServerAddress::parse(" "), Err(AddressError::Empty));
        assert!(ServerAddress::parse("host:abc").is_err());
        assert!(ServerAddress::parse("host::70000").is_err());
        assert_eq!(
            ServerAddress::parse("[::1"),
            Err(AddressError::UnterminatedBracket)
        );
    }

    #[test]
    fn display_name_round_trips() {
        for input in ["pop-os", "pop-os:1", "pop-os::2222", "[::1]:4"] {
            let parsed = ServerAddress::parse(input).unwrap();
            assert_eq!(
                ServerAddress::parse(&parsed.display_name()).unwrap(),
                parsed
            );
        }
    }
}
