//! defrag:// deep-link parsing + engine launch.
//!
//! The web (defrag.racing) emits server-join links of the form
//! `defrag://<host>:<port>`. Host can be an IPv4, an IPv6 in brackets
//! (`[::1]:27960`), or a DNS hostname (`deimos.baseq.fr:27950`) -
//! the Q3 engine resolves DNS itself, so we just pass the literal
//! through. Anything else we treat as garbage and surface a toast in
//! the UI instead of guessing.
//!
//! Why Command::new directly rather than tauri-plugin-shell::open: the
//! shell plugin invokes the registered handler for the file (which on
//! Windows means Explorer's notion of "what opens .exe", usually just
//! the exe itself but with no control over argv). We need to pass
//! `+connect <host>:<port>` as a Q3 engine cmdline argument, which
//! only works via direct process spawn.

use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("URL is not a defrag:// link: {0}")]
    WrongScheme(String),

    #[error("URL has no host:port - expected defrag://<host>:<port>, got {0}")]
    MissingHost(String),

    #[error("\"{0}\" is not a valid host:port - {1}")]
    BadAddress(String, String),

    #[error("engine binary not configured - pick one in Settings first")]
    EngineNotConfigured,

    #[error("engine binary {0} does not exist (it may have been moved or uninstalled)")]
    EngineMissing(std::path::PathBuf),

    #[error("failed to spawn engine: {0}")]
    SpawnFailed(#[from] std::io::Error),
}

/// Parsed `host:port` from a defrag:// URL. Host is kept as a literal
/// string (no DNS resolution here) so hostnames pass through unchanged
/// to the engine; the engine's own resolver handles them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddr {
    host: String,
    port: u16,
}

impl ServerAddr {
    pub fn host(&self) -> &str { &self.host }
    pub fn port(&self) -> u16 { self.port }
}

impl std::fmt::Display for ServerAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Re-bracket IPv6 (contains ':') for the engine's +connect arg,
        // so the engine doesn't mistake the last colon in the address
        // for a port separator.
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Parses a `defrag://<host>:<port>` URL.
///
/// Accepts IPv4, IPv6 (bracketed), and DNS hostnames. We do not
/// resolve DNS - the Q3 engine does that at connect time. Validation
/// is intentionally light: non-empty host, no whitespace, port in
/// range. Anything that survives that gets passed to the engine,
/// which will surface its own error if the hostname turns out to be
/// bogus.
pub fn parse_url(url: &str) -> Result<ServerAddr, ProtocolError> {
    let trimmed = url.trim();
    let after_scheme = trimmed
        .strip_prefix("defrag://")
        .ok_or_else(|| ProtocolError::WrongScheme(trimmed.to_string()))?;

    // Strip any trailing slash or query string the OS / browser may
    // have appended (Chrome on Linux tends to add a `/`).
    let host_port = after_scheme
        .trim_end_matches('/')
        .split(|c| c == '?' || c == '#')
        .next()
        .unwrap_or("");

    if host_port.is_empty() {
        return Err(ProtocolError::MissingHost(trimmed.to_string()));
    }

    // IPv6 bracketed form: [::1]:27960. Find the closing ']' and the
    // colon immediately after it.
    let (host, port_str) = if let Some(rest) = host_port.strip_prefix('[') {
        let close = rest.find(']').ok_or_else(|| {
            ProtocolError::BadAddress(host_port.to_string(), "unterminated IPv6 bracket".into())
        })?;
        let host = &rest[..close];
        let after = &rest[close + 1..];
        let port = after.strip_prefix(':').ok_or_else(|| {
            ProtocolError::BadAddress(host_port.to_string(), "missing port after IPv6 bracket".into())
        })?;
        (host, port)
    } else {
        // IPv4 or hostname: split at the LAST colon. Hostnames can
        // contain dots but not colons, IPv4 has no colons at all.
        let idx = host_port.rfind(':').ok_or_else(|| {
            ProtocolError::BadAddress(host_port.to_string(), "missing port".into())
        })?;
        (&host_port[..idx], &host_port[idx + 1..])
    };

    if host.is_empty() {
        return Err(ProtocolError::BadAddress(host_port.to_string(), "empty host".into()));
    }
    if host.chars().any(|c| c.is_whitespace()) {
        return Err(ProtocolError::BadAddress(host_port.to_string(), "host contains whitespace".into()));
    }
    let port: u16 = port_str.parse().map_err(|_| {
        ProtocolError::BadAddress(host_port.to_string(), format!("invalid port \"{}\"", port_str))
    })?;
    if port == 0 {
        return Err(ProtocolError::BadAddress(host_port.to_string(), "port is 0".into()));
    }

    Ok(ServerAddr { host: host.to_string(), port })
}

/// Spawns the configured engine with `+connect <host>:<port>`. Returns
/// once the process is started - we don't wait for it to exit because
/// the engine stays open for the entire gaming session.
pub fn launch(engine: Option<&Path>, addr: &ServerAddr) -> Result<(), ProtocolError> {
    spawn_engine(engine, |cmd| {
        // Q3-family engines parse `+connect <host>:<port>` as a startup
        // console command. Two args, not one - `+connect host:port` as
        // a single string is silently ignored by the engine.
        cmd.arg("+connect").arg(addr.to_string());
    })
}

/// Spawns the configured engine without any `+connect` - just opens
/// Defrag at the main menu. Used by the Dashboard "Play" button so a
/// user who keeps the launcher in their tray can jump into the game
/// without finding the engine .exe in their filesystem.
pub fn launch_no_connect(engine: Option<&Path>) -> Result<(), ProtocolError> {
    spawn_engine(engine, |_| {})
}

fn spawn_engine(
    engine: Option<&Path>,
    extra_args: impl FnOnce(&mut Command),
) -> Result<(), ProtocolError> {
    let engine = engine.ok_or(ProtocolError::EngineNotConfigured)?;
    if !engine.exists() {
        return Err(ProtocolError::EngineMissing(engine.to_path_buf()));
    }

    let mut cmd = Command::new(engine);
    extra_args(&mut cmd);

    // Set CWD to the engine's directory. oDFe/iDFe load fs_basepath
    // relative to CWD on Linux, and starting the engine from a random
    // directory makes it look in the wrong place for pak files.
    if let Some(dir) = engine.parent() {
        cmd.current_dir(dir);
    }

    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_ipv4() {
        let addr = parse_url("defrag://1.2.3.4:27960").unwrap();
        assert_eq!(addr.host(), "1.2.3.4");
        assert_eq!(addr.port(), 27960);
        assert_eq!(addr.to_string(), "1.2.3.4:27960");
    }

    #[test]
    fn parses_with_trailing_slash() {
        let addr = parse_url("defrag://1.2.3.4:27960/").unwrap();
        assert_eq!(addr.to_string(), "1.2.3.4:27960");
    }

    #[test]
    fn parses_ipv6_bracket_form() {
        let addr = parse_url("defrag://[::1]:27960").unwrap();
        assert_eq!(addr.host(), "::1");
        assert_eq!(addr.port(), 27960);
        assert_eq!(addr.to_string(), "[::1]:27960");
    }

    #[test]
    fn parses_hostname() {
        // Engine resolves DNS itself; we just pass the hostname through.
        let addr = parse_url("defrag://deimos.baseq.fr:27950").unwrap();
        assert_eq!(addr.host(), "deimos.baseq.fr");
        assert_eq!(addr.port(), 27950);
        assert_eq!(addr.to_string(), "deimos.baseq.fr:27950");
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(matches!(parse_url("http://1.2.3.4:27960"), Err(ProtocolError::WrongScheme(_))));
    }

    #[test]
    fn rejects_missing_port() {
        assert!(matches!(parse_url("defrag://deimos.baseq.fr"), Err(ProtocolError::BadAddress(_, _))));
    }

    #[test]
    fn rejects_bad_port() {
        assert!(matches!(parse_url("defrag://1.2.3.4:99999"), Err(ProtocolError::BadAddress(_, _))));
        assert!(matches!(parse_url("defrag://1.2.3.4:0"), Err(ProtocolError::BadAddress(_, _))));
        assert!(matches!(parse_url("defrag://1.2.3.4:abc"), Err(ProtocolError::BadAddress(_, _))));
    }

    #[test]
    fn rejects_empty_host() {
        assert!(matches!(parse_url("defrag://"), Err(ProtocolError::MissingHost(_))));
        assert!(matches!(parse_url("defrag://:27960"), Err(ProtocolError::BadAddress(_, _))));
    }
}
