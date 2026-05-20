//! defrag:// deep-link parsing + engine launch.
//!
//! The web (defrag.racing) emits server-join links of the form
//! `defrag://1.2.3.4:27960`. There's exactly one variant — no map links,
//! no demo playback, no query string. Anything else we treat as garbage
//! and surface a toast in the UI instead of guessing.
//!
//! Why std::net::SocketAddr for parsing instead of a regex: it correctly
//! handles IPv6 (`[::1]:27960`), rejects out-of-range octets, and
//! validates port range. A naive `split(':')` would silently accept
//! `defrag://foo:99999` or `defrag://1.2.3:27960`.
//!
//! Why Command::new directly rather than tauri-plugin-shell::open: the
//! shell plugin invokes the registered handler for the file (which on
//! Windows means Explorer's notion of "what opens .exe", usually just
//! the exe itself but with no control over argv). We need to pass
//! `+connect <ip>:<port>` as a Q3 engine cmdline argument, which only
//! works via direct process spawn.

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("URL is not a defrag:// link: {0}")]
    WrongScheme(String),

    #[error("URL has no host:port — expected defrag://<ip>:<port>, got {0}")]
    MissingHost(String),

    #[error("\"{0}\" is not a valid ip:port — {1}")]
    BadSocketAddr(String, std::net::AddrParseError),

    #[error("engine binary not configured — pick one in Settings first")]
    EngineNotConfigured,

    #[error("engine binary {0} does not exist (it may have been moved or uninstalled)")]
    EngineMissing(std::path::PathBuf),

    #[error("failed to spawn engine: {0}")]
    SpawnFailed(#[from] std::io::Error),
}

/// Parses a `defrag://<ip>:<port>` URL into a SocketAddr.
///
/// Rejects anything that isn't strictly a defrag-scheme URL with a valid
/// host:port. We're deliberately strict: a user typing the URL by hand
/// will see a clear error rather than the launcher silently doing
/// nothing.
pub fn parse_url(url: &str) -> Result<SocketAddr, ProtocolError> {
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

    host_port
        .parse::<SocketAddr>()
        .map_err(|e| ProtocolError::BadSocketAddr(host_port.to_string(), e))
}

/// Spawns the configured engine with `+connect <ip>:<port>`. Returns
/// once the process is started — we don't wait for it to exit because
/// the engine stays open for the entire gaming session.
pub fn launch(engine: Option<&Path>, addr: SocketAddr) -> Result<(), ProtocolError> {
    let engine = engine.ok_or(ProtocolError::EngineNotConfigured)?;
    if !engine.exists() {
        return Err(ProtocolError::EngineMissing(engine.to_path_buf()));
    }

    // Q3-family engines parse `+connect <ip>:<port>` as a startup
    // console command. Two args, not one — `+connect ip:port` as a
    // single string is silently ignored by the engine.
    let mut cmd = Command::new(engine);
    cmd.arg("+connect").arg(addr.to_string());

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
        assert_eq!(addr.to_string(), "1.2.3.4:27960");
    }

    #[test]
    fn parses_with_trailing_slash() {
        // Some browsers (Chrome on Linux) append a slash to deep-link URLs.
        let addr = parse_url("defrag://1.2.3.4:27960/").unwrap();
        assert_eq!(addr.to_string(), "1.2.3.4:27960");
    }

    #[test]
    fn parses_ipv6_bracket_form() {
        let addr = parse_url("defrag://[::1]:27960").unwrap();
        assert_eq!(addr.port(), 27960);
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(matches!(
            parse_url("http://1.2.3.4:27960"),
            Err(ProtocolError::WrongScheme(_))
        ));
    }

    #[test]
    fn rejects_missing_port() {
        assert!(matches!(
            parse_url("defrag://1.2.3.4"),
            Err(ProtocolError::BadSocketAddr(_, _))
        ));
    }

    #[test]
    fn rejects_bad_port() {
        assert!(matches!(
            parse_url("defrag://1.2.3.4:99999"),
            Err(ProtocolError::BadSocketAddr(_, _))
        ));
    }

    #[test]
    fn rejects_hostname_instead_of_ip() {
        // Q3 master is IP-based; a hostname like defrag.racing:27960 isn't
        // a valid SocketAddr and shouldn't slip through.
        assert!(matches!(
            parse_url("defrag://defrag.racing:27960"),
            Err(ProtocolError::BadSocketAddr(_, _))
        ));
    }

    #[test]
    fn rejects_empty_host() {
        assert!(matches!(
            parse_url("defrag://"),
            Err(ProtocolError::MissingHost(_))
        ));
    }
}
