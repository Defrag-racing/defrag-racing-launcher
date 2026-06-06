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
/// the child process handle once started - we don't wait for it to exit
/// because the engine stays open for the entire gaming session, but the
/// caller keeps the handle so the map-history tracker can watch the
/// process and stop tracking when the game closes.
pub fn launch(engine: Option<&Path>, addr: &ServerAddr) -> Result<std::process::Child, ProtocolError> {
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
/// without finding the engine .exe in their filesystem. No server, so no
/// session tracking - the child handle is dropped.
pub fn launch_no_connect(engine: Option<&Path>) -> Result<(), ProtocolError> {
    spawn_engine(engine, |_| {}).map(|_| ())
}

/// Spawns the engine with caller-supplied extra arguments (no
/// `+connect`). Backs the developer-mode custom Quick launch + named
/// launch profiles + the Maps offline-run buttons: the args come from the
/// caller, already split into tokens via `split_args`. No server, so the
/// child handle is dropped.
pub fn launch_with_args(engine: Option<&Path>, args: &[String]) -> Result<(), ProtocolError> {
    spawn_engine(engine, |cmd| {
        for a in args {
            cmd.arg(a);
        }
    })
    .map(|_| ())
}

/// Open an external URL in the user's browser.
///
/// Linux: we deliberately spawn the system opener ourselves instead of
/// going through `tauri-plugin-opener`, for the same reason the engine
/// launch needs special handling - when the launcher runs as an AppImage,
/// the opener inherits the AppImage's mangled `LD_LIBRARY_PATH` and the
/// browser it ultimately launches loads the bundle's libraries and fails
/// to start. We strip the AppImage additions (`strip_appimage_env`) so the
/// browser comes up with a clean, shell-equivalent environment, and walk a
/// fallback chain (xdg-open -> gio -> common browsers) so it works across
/// desktops. A spawn that starts the process is treated as success.
#[cfg(target_os = "linux")]
pub fn open_external_url(_app: &tauri::AppHandle, target: &str) -> Result<(), String> {
    // (binary, leading args before the URL)
    let candidates: &[(&str, &[&str])] = &[
        ("xdg-open", &[]),
        ("gio", &["open"]),
        ("x-www-browser", &[]),
        ("www-browser", &[]),
        ("sensible-browser", &[]),
        ("firefox", &[]),
        ("google-chrome", &[]),
        ("chromium", &[]),
        ("chromium-browser", &[]),
    ];

    let mut last_err = String::from("no opener found on PATH");
    for (bin, pre) in candidates {
        let mut cmd = Command::new(bin);
        for p in *pre {
            cmd.arg(p);
        }
        cmd.arg(target);
        strip_appimage_env(&mut cmd);
        match cmd.spawn() {
            Ok(_) => return Ok(()),
            Err(e) => last_err = format!("{bin}: {e}"),
        }
    }
    Err(format!("could not open URL ({last_err})"))
}

/// Non-Linux: the opener plugin works fine (no AppImage env to undo), so
/// defer to it.
#[cfg(not(target_os = "linux"))]
pub fn open_external_url(app: &tauri::AppHandle, target: &str) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(target.to_string(), None::<String>)
        .map_err(|e| e.to_string())
}

/// Split a free-form argument string into individual arguments the way a
/// shell would, honouring single and double quotes so a value containing
/// a space (e.g. `+set fs_game "my mod"`) survives as one argument.
/// Backslash escaping is intentionally not supported - Q3 engine args
/// don't need it and it keeps the rule "what you'd type on a command
/// line" easy to reason about. Empty / whitespace-only input yields an
/// empty vec.
pub fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;

    for c in input.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_token {
                    args.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(cur);
    }
    args
}

/// Spawn the engine and return the child handle so the caller can track
/// the process lifetime (used by the map-history tracker to know how long
/// the user is in-game). Callers that don't care just drop it.
fn spawn_engine(
    engine: Option<&Path>,
    extra_args: impl FnOnce(&mut Command),
) -> Result<std::process::Child, ProtocolError> {
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

    // On Linux, when the launcher itself runs as an AppImage, hand the
    // engine a clean environment so it loads system libraries instead of
    // the AppImage's bundled ones (see strip_appimage_env).
    #[cfg(target_os = "linux")]
    strip_appimage_env(&mut cmd);

    Ok(cmd.spawn()?)
}

/// Undo the environment mangling an AppImage's `AppRun` does, for a child
/// process we spawn (the Q3 engine).
///
/// When the launcher is distributed as an AppImage, its AppRun prepends
/// `$APPDIR/usr/lib` onto `LD_LIBRARY_PATH` and points a handful of
/// module-loader vars at bundled copies, so that the launcher's own
/// binary finds the libraries packed beside it. Any process the launcher
/// spawns inherits all of that and then resolves shared libraries
/// (libcurl, glib, …) to the AppImage's bundled versions, which are built
/// for the launcher and routinely mismatch what the engine needs - the
/// engine misbehaves when launched from the launcher but works when the
/// user runs the exact same path by hand from a shell (clean env).
///
/// We restore the shell-equivalent environment by removing only what
/// AppRun added: every colon-separated entry under `$APPDIR` is dropped
/// from the path-like vars (and the var removed entirely if nothing
/// genuine remains), and the bundle-pointing loader vars are cleared.
/// Values the user actually set survive untouched. No-op when not running
/// from an AppImage (`APPDIR` unset).
#[cfg(target_os = "linux")]
fn strip_appimage_env(cmd: &mut Command) {
    use std::path::PathBuf;

    let appdir = match std::env::var_os("APPDIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => return, // not an AppImage launch - nothing AppRun touched
    };

    // Path-list vars AppRun prepends $APPDIR entries onto. Keep the user's
    // own entries, strip the bundle's.
    const PATH_VARS: &[&str] = &[
        "LD_LIBRARY_PATH",
        "PATH",
        "XDG_DATA_DIRS",
        "GST_PLUGIN_SYSTEM_PATH",
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "GTK_PATH",
        "QT_PLUGIN_PATH",
        "PYTHONPATH",
        "PERLLIB",
        "GSETTINGS_SCHEMA_DIR",
        "GIO_EXTRA_MODULES",
    ];
    for var in PATH_VARS {
        if let Some(val) = std::env::var_os(var) {
            let kept: Vec<PathBuf> = std::env::split_paths(&val)
                .filter(|p| !p.starts_with(&appdir))
                .collect();
            if kept.is_empty() {
                cmd.env_remove(var);
            } else if let Ok(joined) = std::env::join_paths(kept) {
                cmd.env(var, joined);
            }
        }
    }

    // Vars AppRun points straight at the bundle - no system value to
    // preserve, so clear them for the child outright. APPDIR/APPIMAGE/
    // ARGV0/OWD are AppImage runtime breadcrumbs that have no business
    // leaking into the engine either.
    const CLEAR_VARS: &[&str] = &[
        "LD_PRELOAD",
        "GDK_PIXBUF_MODULE_FILE",
        "GDK_PIXBUF_MODULEDIR",
        "GIO_MODULE_DIR",
        "GTK_IM_MODULE_FILE",
        "APPDIR",
        "APPIMAGE",
        "ARGV0",
        "OWD",
    ];
    for var in CLEAR_VARS {
        cmd.env_remove(var);
    }
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

    #[test]
    fn split_args_basic() {
        assert_eq!(split_args("+set r_fullscreen 0"), vec!["+set", "r_fullscreen", "0"]);
    }

    #[test]
    fn split_args_empty_and_whitespace() {
        assert!(split_args("").is_empty());
        assert!(split_args("   \t ").is_empty());
    }

    #[test]
    fn split_args_respects_quotes() {
        assert_eq!(
            split_args(r#"+set fs_game "my mod" +exec 'cfg one'"#),
            vec!["+set", "fs_game", "my mod", "+exec", "cfg one"],
        );
    }

    #[test]
    fn split_args_collapses_runs_of_spaces() {
        assert_eq!(split_args("a    b\tc"), vec!["a", "b", "c"]);
    }
}
