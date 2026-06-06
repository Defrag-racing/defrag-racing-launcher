//! Per-session server map tracking.
//!
//! When the launcher starts the engine with `+connect <server>`, it hands
//! the resulting child process here. While that process is alive we poll
//! the live server list in the background (even with the launcher hidden
//! in the tray) and append every new map the server rotates to onto the
//! matching connection-history entry. When the game exits, the process is
//! reaped and tracking for that session stops. The user's mental model:
//! "log the maps I played on the server, for as long as I'm in the game".
//!
//! Needs a launcher token (the server list is token-locked); without one
//! the poll simply finds nothing to read and the map history stays empty.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::history::{self, ConnectionHistory};

/// How often to re-read the server list. Map rotations happen on the order
/// of minutes, so 45s gives good resolution without hammering the API.
const POLL_INTERVAL_SECS: u64 = 45;

struct Session {
    /// The running engine process. `try_wait()` tells us when the game
    /// closed so we can stop tracking this session.
    child: std::process::Child,
    /// Host + port exactly as we connected (matched against the live
    /// server list's ip/port). A hostname connect that the list reports by
    /// numeric IP simply won't match - acceptable, map history is
    /// best-effort.
    host: String,
    port: u16,
    /// The connection-history entry id to append maps to.
    session_id: String,
    /// Last map we recorded for this session, to skip duplicates while the
    /// server stays on the same map.
    last_map: Option<String>,
}

struct State {
    sessions: Vec<Session>,
    /// Whether the background poll task is currently running. Guarded by
    /// the same mutex as `sessions` so starting/stopping the task can't
    /// race a register().
    running: bool,
}

pub struct SessionTracker {
    state: Mutex<State>,
    history: Arc<ConnectionHistory>,
}

impl SessionTracker {
    pub fn new(history: Arc<ConnectionHistory>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State { sessions: Vec::new(), running: false }),
            history,
        })
    }

    /// Begin tracking a connected session. `seed_map` is the map known at
    /// connect time (already logged on the history entry) so we don't
    /// re-append it on the first poll. Spawns the poll task if it isn't
    /// already running.
    pub fn register(
        self: &Arc<Self>,
        child: std::process::Child,
        host: String,
        port: u16,
        session_id: String,
        seed_map: Option<String>,
    ) {
        let mut st = self.state.lock().unwrap();
        st.sessions.push(Session { child, host, port, session_id, last_map: seed_map });
        if !st.running {
            st.running = true;
            drop(st);
            let this = Arc::clone(self);
            tauri::async_runtime::spawn(async move { this.run().await });
        }
    }

    async fn run(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;

            // Reap exited processes and snapshot what's still active. The
            // guard is dropped before any await below (std Mutex guards
            // aren't Send). If nothing is left, mark the task stopped under
            // the lock and exit - a register() racing in will see
            // running=false and spawn a fresh task.
            let active: Vec<(String, u16, String, Option<String>)> = {
                let mut st = self.state.lock().unwrap();
                st.sessions.retain_mut(|s| matches!(s.child.try_wait(), Ok(None)));
                if st.sessions.is_empty() {
                    st.running = false;
                    return;
                }
                st.sessions
                    .iter()
                    .map(|s| (s.host.clone(), s.port, s.session_id.clone(), s.last_map.clone()))
                    .collect()
            };

            // Read the live server list once for all active sessions.
            let Ok(Some(token)) = crate::token::load() else { continue };
            let Ok(client) = crate::api::Client::new(crate::config::api_base_url(), token) else { continue };
            let Ok(json) = client.fetch_servers().await else { continue };
            let servers = match json.get("servers").and_then(|v| v.as_array()) {
                Some(arr) => arr.clone(),
                None => continue,
            };

            for (host, port, session_id, last_map) in active {
                let found = servers.iter().find(|s| {
                    s.get("port").and_then(|p| p.as_u64()) == Some(port as u64)
                        && s.get("ip").and_then(|i| i.as_str()) == Some(host.as_str())
                });
                let Some(sv) = found else { continue };
                let Some(map) = sv.get("map").and_then(|m| m.as_str()) else { continue };
                if last_map.as_deref() == Some(map) {
                    continue;
                }
                let physics = sv.get("defrag").and_then(|d| d.as_str()).map(|s| s.to_string());
                self.history.append_map(&session_id, map.to_string(), physics, history::now_ms());

                // Remember it so we don't re-log until the map changes again.
                let mut st = self.state.lock().unwrap();
                if let Some(s) = st.sessions.iter_mut().find(|s| s.session_id == session_id) {
                    s.last_map = Some(map.to_string());
                }
            }
        }
    }
}
