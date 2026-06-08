//! Embedded demo player (Windows only).
//!
//! Plays a Quake3 `.dm_68` Defrag demo *inside* the launcher window. We ship a
//! purpose-built oDFe engine build (resources/odfe/) that can render into a
//! launcher-supplied child window and be driven over a loopback control
//! channel - see the engine's `code/win32/win_glimp.c` (`in_embedParent`) and
//! `code/client/cl_control.c` (`in_controlPort`).
//!
//! How a session works:
//!   1. We create a native WS_CHILD "stage" window inside the launcher's main
//!      window, sized to the demo's correct aspect (letterboxed - the webview
//!      paints black around it). The engine renders r_mode-independently into
//!      whatever client rect its parent has, so the stage *is* the render area.
//!   2. We spawn the bundled engine with `+set in_embedParent <stage hwnd>`
//!      (so it becomes a child of the stage) and `+set in_controlPort <port>`
//!      (so it opens a loopback control listener), plus `+demo <file>`.
//!   3. A background thread connects to that control port, forwards transport
//!      commands the frontend issues (`timescale`, `demopause`, `seekdemo`,
//!      `vid_restart`) and parses the engine's periodic `status ...` line back
//!      into `demo-player-status` events the Vue view renders as a playhead.
//!
//! On window resize the launcher repositions the stage and issues `vid_restart`;
//! the engine destroys+recreates its child window at the new parent size (the
//! map stays loaded, so it's cheap). Everything Win32 happens on the main
//! thread via `run_on_main_thread`, so the stage shares Tauri's message pump.
//!
//! On non-Windows platforms the whole feature is a no-op that returns an error -
//! the engine binary we embed is Windows-only.

// On non-Windows the session helpers below are only reachable through the
// Windows-gated `demo_player_start` body, so they read as dead code there.
// Allow it off-Windows only, so the primary (Windows) target still flags real
// dead code.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::AppState;

/// Live playback session. Present only while a demo is loaded.
struct Session {
    /// The engine process. Shared with the control thread so it can notice the
    /// user closing the engine; `stop()` kills it from the command side.
    child: Arc<Mutex<std::process::Child>>,
    /// Tells the control thread to exit (set on stop / replace).
    stop: Arc<AtomicBool>,
    /// Outgoing console lines to the engine. Buffered until the control thread
    /// connects, so a seek issued right after start still lands.
    cmd_tx: Sender<String>,
    /// Native stage window handle (as isize; 0 on non-Windows). Destroyed on stop.
    stage: isize,
    /// The launcher's main window HWND (owner of the stage), so reposition can
    /// re-map the stage's client rect to screen coordinates after the launcher
    /// is moved.
    parent: isize,
    join: Option<JoinHandle<()>>,
}

/// App-state holder for the (at most one) active demo player session.
#[derive(Default)]
pub struct DemoPlayer {
    inner: Mutex<Option<Session>>,
}

/// Status snapshot pushed to the frontend ~10x/sec while a demo plays.
/// Mirrors the engine's `status time <t> start <s> total <T> demo <d> paused <p>
/// atend <a>` line. The playhead position is `time - start`; the demo length is
/// `total - start` (total is 0 until the launcher seeks once to measure it).
#[derive(Clone, serde::Serialize)]
struct StatusEvent {
    time: i32,
    start: i32,
    total: i32,
    demo: bool,
    paused: bool,
    atend: bool,
}

// ---- letterbox math --------------------------------------------------------

/// Given an available region (physical pixels, relative to the main window's
/// client area) and the demo's aspect ratio, return the aspect-correct,
/// centered sub-rect the stage window should occupy. The webview paints the
/// region black, so the leftover margins read as letterbox/pillarbox bars.
fn letterbox(rx: i32, ry: i32, rw: i32, rh: i32, aspect: f32) -> (i32, i32, i32, i32) {
    if rw <= 0 || rh <= 0 || !(aspect > 0.0) {
        return (rx, ry, rw.max(1), rh.max(1));
    }
    let region_aspect = rw as f32 / rh as f32;
    let (sw, sh) = if region_aspect > aspect {
        // region is wider than the demo -> pillarbox (limit by height)
        let sh = rh;
        let sw = (rh as f32 * aspect).round() as i32;
        (sw, sh)
    } else {
        // region is taller than the demo -> letterbox (limit by width)
        let sw = rw;
        let sh = (rw as f32 / aspect).round() as i32;
        (sw, sh)
    };
    let sx = rx + (rw - sw) / 2;
    let sy = ry + (rh - sh) / 2;
    (sx, sy, sw.max(1), sh.max(1))
}

/// Derive `(fs_basepath, fs_game, demo_arg)` from a demo's absolute path. Defrag
/// demos live at `<base>/<game>/demos/<sub...>/<file>.dm_68`; the engine plays
/// `+demo <demo_arg>` with `fs_basepath=<base>` / `fs_game=<game>`, where
/// `demo_arg` is the path relative to the `demos` folder (forward-slashed). The
/// nearest ancestor named `demos` defines the layout, so demos in subfolders
/// work too. Errors if the demo isn't inside a `demos` folder (the engine can't
/// load it then).
fn derive_demo_launch(demo: &std::path::Path) -> Result<(std::path::PathBuf, String, String), String> {
    let demos_dir = demo
        .ancestors()
        .skip(1) // skip the file itself
        .find(|a| {
            a.file_name()
                .map_or(false, |n| n.eq_ignore_ascii_case("demos"))
        })
        .ok_or_else(|| "This demo isn't inside a 'demos' folder, so the engine can't load it.".to_string())?;
    let game_dir = demos_dir
        .parent()
        .ok_or_else(|| "Could not resolve the game folder from the demo path.".to_string())?;
    let base = game_dir
        .parent()
        .ok_or_else(|| "Could not resolve the install folder from the demo path.".to_string())?;
    let fs_game = game_dir
        .file_name()
        .ok_or_else(|| "Could not resolve fs_game from the demo path.".to_string())?
        .to_string_lossy()
        .to_string();
    let demo_arg = demo
        .strip_prefix(demos_dir)
        .map_err(|_| "Could not resolve the demo path.".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    Ok((base.to_path_buf(), fs_game, demo_arg))
}

// ---- native stage window ---------------------------------------------------
//
// The stage is an OWNED top-level WS_POPUP window (not a child) placed over the
// launcher's client area. A child window would be composited *behind* the
// WebView2 (Edge draws its content over sibling/child HWNDs via DirectComp), so
// the demo played but the area stayed black. An owned top-level window is not
// part of that composition and always renders above its owner, so the engine
// shows. The trade-off: it doesn't move with the window automatically, so the
// frontend re-sends its rect on window move/resize (reposition / set_region).
//
// All Win32 calls MUST be invoked on the UI thread (see `*_on_main`).

#[cfg(windows)]
mod stage {
    use std::ffi::c_void;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::{ClientToScreen, GetStockObject, BLACK_BRUSH, HBRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, HMENU,
        HWND_TOP, SWP_NOACTIVATE, WINDOW_EX_STYLE, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };

    const CLASS_NAME: PCWSTR = w!("oDFeDemoStage");

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    fn ensure_class() {
        unsafe {
            let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let brush = HBRUSH(GetStockObject(BLACK_BRUSH).0);
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: CLASS_NAME,
                hbrBackground: brush,
                ..Default::default()
            };
            // Re-registering an existing class just returns 0 - harmless, we
            // only need it registered once per process.
            RegisterClassW(&wc);
        }
    }

    // Map a point in `owner`'s client area (physical px) to screen coordinates.
    unsafe fn to_screen(owner: HWND, x: i32, y: i32) -> (i32, i32) {
        let mut pt = POINT { x, y };
        let _ = ClientToScreen(owner, &mut pt);
        (pt.x, pt.y)
    }

    /// Create the owned top-level stage window over `owner`'s client area at the
    /// client-relative rect (cx, cy, w, h). Returns 0 on failure.
    pub fn create(owner: isize, cx: i32, cy: i32, w: i32, h: i32) -> isize {
        ensure_class();
        unsafe {
            let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let owner = HWND(owner as *mut c_void);
            let (sx, sy) = to_screen(owner, cx, cy);
            match CreateWindowExW(
                // tool window = no taskbar button; noactivate = clicking the
                // demo doesn't steal focus from the launcher's transport UI.
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_NAME,
                w!(""),
                WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
                sx,
                sy,
                w,
                h,
                Some(owner), // owner (WS_POPUP => owned top-level, not a child)
                None::<HMENU>,
                Some(hinstance),
                None,
            ) {
                Ok(hwnd) => hwnd.0 as isize,
                Err(_) => 0,
            }
        }
    }

    /// Move/resize the stage to a new client-relative rect of `owner`.
    pub fn reposition(owner: isize, hwnd: isize, cx: i32, cy: i32, w: i32, h: i32) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            let owner = HWND(owner as *mut c_void);
            let hwnd = HWND(hwnd as *mut c_void);
            let (sx, sy) = to_screen(owner, cx, cy);
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), sx, sy, w, h, SWP_NOACTIVATE);
        }
    }

    pub fn destroy(hwnd: isize) {
        if hwnd == 0 {
            return;
        }
        unsafe {
            let hwnd = HWND(hwnd as *mut c_void);
            let _ = DestroyWindow(hwnd);
        }
    }
}

#[cfg(not(windows))]
mod stage {
    pub fn create(_owner: isize, _cx: i32, _cy: i32, _w: i32, _h: i32) -> isize {
        0
    }
    pub fn reposition(_owner: isize, _hwnd: isize, _cx: i32, _cy: i32, _w: i32, _h: i32) {}
    pub fn destroy(_hwnd: isize) {}
}

/// Run `stage::create` on the UI thread and wait for the resulting handle.
fn create_stage_on_main(app: &AppHandle, owner: isize, cx: i32, cy: i32, w: i32, h: i32) -> isize {
    let (tx, rx) = mpsc::channel::<isize>();
    if app
        .run_on_main_thread(move || {
            let _ = tx.send(stage::create(owner, cx, cy, w, h));
        })
        .is_err()
    {
        return 0;
    }
    rx.recv_timeout(Duration::from_secs(2)).unwrap_or(0)
}

fn reposition_stage_on_main(app: &AppHandle, owner: isize, hwnd: isize, cx: i32, cy: i32, w: i32, h: i32) {
    let _ = app.run_on_main_thread(move || stage::reposition(owner, hwnd, cx, cy, w, h));
}

fn destroy_stage_on_main(app: &AppHandle, hwnd: isize) {
    let _ = app.run_on_main_thread(move || stage::destroy(hwnd));
}

// ---- control channel -------------------------------------------------------

/// Parse one `status ...` line into a `StatusEvent`. Returns None for any other
/// line. Format: `status time <i> start <i> total <i> demo <i> paused <i> atend <i>`.
fn parse_status(line: &str) -> Option<StatusEvent> {
    let mut it = line.split_whitespace();
    if it.next()? != "status" {
        return None;
    }
    // Read key/value pairs in any order, tolerating unknown keys.
    let mut time = 0;
    let mut start = 0;
    let mut total = 0;
    let mut demo = 0;
    let mut paused = 0;
    let mut atend = 0;
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        let n: i32 = v.parse().unwrap_or(0);
        match k {
            "time" => time = n,
            "start" => start = n,
            "total" => total = n,
            "demo" => demo = n,
            "paused" => paused = n,
            "atend" => atend = n,
            _ => {}
        }
    }
    Some(StatusEvent {
        time,
        start,
        total,
        demo: demo != 0,
        paused: paused != 0,
        atend: atend != 0,
    })
}

/// Background loop: connect to the engine's control listener (retrying until it
/// comes up), forward queued console commands, and surface status lines as
/// `demo-player-status` events. Exits when `stop` is set or the engine process
/// is gone, emitting `demo-player-closed` in the latter case.
fn run_control(
    app: AppHandle,
    port: u16,
    child: Arc<Mutex<std::process::Child>>,
    cmd_rx: Receiver<String>,
    stop: Arc<AtomicBool>,
) {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut acc = String::new();

    'session: loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // Has the engine exited before we ever connected? Then we're done.
        if child_exited(&child) {
            app.emit("demo-player-closed", ()).ok();
            return;
        }

        // (Re)connect. The engine opens its listener lazily on its first frame,
        // so connection refused early on is expected - just retry.
        let stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
            Ok(s) => s,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(120));
                continue;
            }
        };
        stream.set_nonblocking(true).ok();
        let mut stream = stream;
        let mut buf = [0u8; 2048];

        loop {
            if stop.load(Ordering::Relaxed) {
                break 'session;
            }
            if child_exited(&child) {
                app.emit("demo-player-closed", ()).ok();
                return;
            }

            // Flush any queued outgoing commands.
            while let Ok(line) = cmd_rx.try_recv() {
                let line = format!("{}\n", line.trim_end());
                if stream.write_all(line.as_bytes()).is_err() {
                    // connection died - drop back to the reconnect loop
                    continue 'session;
                }
            }

            // Drain whatever the engine has sent.
            match stream.read(&mut buf) {
                Ok(0) => {
                    // peer closed - reconnect (engine may have done a vid_restart
                    // that briefly tore the listener down, or it's shutting down)
                    std::thread::sleep(Duration::from_millis(120));
                    continue 'session;
                }
                Ok(n) => {
                    acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                    while let Some(nl) = acc.find('\n') {
                        let line: String = acc.drain(..=nl).collect();
                        if let Some(ev) = parse_status(line.trim_end()) {
                            app.emit("demo-player-status", ev).ok();
                        }
                    }
                    // guard against an unbounded line with no newline
                    if acc.len() > 8192 {
                        acc.clear();
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(120));
                    continue 'session;
                }
            }
        }
    }
}

fn child_exited(child: &Arc<Mutex<std::process::Child>>) -> bool {
    if let Ok(mut c) = child.lock() {
        matches!(c.try_wait(), Ok(Some(_)))
    } else {
        false
    }
}

// ---- session lifecycle -----------------------------------------------------

/// Tear down a running session: signal the thread, kill the engine, drop the
/// stage window.
fn stop_session(app: &AppHandle, mut s: Session) {
    s.stop.store(true, Ordering::Relaxed);
    if let Ok(mut c) = s.child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    if let Some(j) = s.join.take() {
        let _ = j.join();
    }
    destroy_stage_on_main(app, s.stage);
}

/// Pick an ephemeral free TCP port for the control channel. Small race window
/// between drop and the engine binding it, made safe by the engine's
/// SO_REUSEADDR; falls back to a fixed port if the probe fails.
fn pick_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(28961)
}

// ---- Tauri commands --------------------------------------------------------

/// Start (or restart) playback of `demo` - a path relative to the engine's
/// `defrag/demos` folder, e.g. `mymap[df.run]00.42.123(player.cz).dm_68`. The
/// stage window is placed at the aspect-correct sub-rect of the region
/// `(x,y,w,h)` (physical pixels in the main window's client area); `aspect`
/// comes from `engine_demo_resolution`. Returns the control port.
// Async so it runs off the main thread: it dispatches the Win32 window work to
// the main thread via `run_on_main_thread` and blocks on the result, which
// would deadlock if the command itself ran on the main thread (Tauri runs sync
// commands there). Same reason for set_region / stop below.
#[tauri::command]
#[cfg_attr(not(windows), allow(unused_variables))]
pub async fn demo_player_start(
    app: AppHandle,
    state: State<'_, AppState>,
    demo: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    aspect: f32,
) -> Result<u16, String> {
    #[cfg(not(windows))]
    {
        Err("The embedded demo player is only available on Windows.".to_string())
    }
    #[cfg(windows)]
    {
        if demo.trim().is_empty() {
            return Err("No demo selected.".to_string());
        }

        // `demo` is the absolute path to a .dm_68. The Defrag layout is
        // <base>/<game>/demos/<sub...>/<file>, so derive fs_basepath=<base>,
        // fs_game=<game>, and the `+demo` arg relative to the demos folder.
        // This lets us play a demo from anywhere the user keeps them, and
        // finds the maps/paks that live in that same install.
        let (basepath, fs_game, demo_arg) =
            derive_demo_launch(std::path::Path::new(&demo))?;

        // Our shipped engine + its renderer DLL live under resources/odfe.
        let exe = app
            .path()
            .resolve("resources/odfe/oDFe.x64.exe", tauri::path::BaseDirectory::Resource)
            .map_err(|e| format!("Could not locate the bundled demo-player engine: {e}"))?;
        if !exe.exists() {
            return Err(format!(
                "Bundled demo-player engine missing at {}",
                exe.display()
            ));
        }
        let exe_dir = exe
            .parent()
            .ok_or_else(|| "Bundled engine path has no parent.".to_string())?
            .to_path_buf();

        // Parent the stage to the launcher's main window.
        let win = app
            .get_webview_window("main")
            .ok_or_else(|| "Main window not available.".to_string())?;
        let parent: isize = win.hwnd().map_err(|e| e.to_string())?.0 as isize;

        // Replace any existing session before creating the new one.
        {
            let mut guard = state.demo_player.inner.lock().unwrap();
            if let Some(old) = guard.take() {
                stop_session(&app, old);
            }
        }

        // Create the aspect-correct stage window (owned popup over `parent`).
        let (sx, sy, sw, sh) = letterbox(x, y, w, h, aspect);
        let stage = create_stage_on_main(&app, parent, sx, sy, sw, sh);
        if stage == 0 {
            return Err("Failed to create the demo render window.".to_string());
        }

        // Spawn the engine embedded in the stage, driven over a control port.
        let port = pick_port();
        let mut cmd = std::process::Command::new(&exe);
        cmd.current_dir(&exe_dir)
            .arg("+set")
            .arg("in_embedParent")
            .arg(stage.to_string())
            .arg("+set")
            .arg("in_controlPort")
            .arg(port.to_string())
            .arg("+set")
            .arg("r_fullscreen")
            .arg("0")
            .arg("+set")
            .arg("con_notifytime")
            .arg("0")
            .arg("+set")
            .arg("fs_basepath")
            .arg(basepath.to_string_lossy().to_string())
            .arg("+set")
            .arg("fs_game")
            .arg(&fs_game)
            .arg("+demo")
            .arg(&demo_arg);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                destroy_stage_on_main(&app, stage);
                return Err(format!("Failed to launch the demo-player engine: {e}"));
            }
        };

        let child = Arc::new(Mutex::new(child));
        let stop = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>();

        let join = {
            let app = app.clone();
            let child = child.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("demo-player-control".into())
                .spawn(move || run_control(app, port, child, cmd_rx, stop))
                .ok()
        };

        let mut guard = state.demo_player.inner.lock().unwrap();
        *guard = Some(Session {
            child,
            stop,
            cmd_tx,
            stage,
            parent,
            join,
        });

        Ok(port)
    }
}

/// Send a verbatim console line to the running engine (transport control:
/// `timescale 0.5`, `demopause 1`, `seekdemo <ms>`, ...). No-op (Ok) if no
/// session is active.
#[tauri::command]
pub fn demo_player_command(state: State<'_, AppState>, line: String) -> Result<(), String> {
    let guard = state.demo_player.inner.lock().unwrap();
    if let Some(s) = guard.as_ref() {
        s.cmd_tx
            .send(line)
            .map_err(|_| "Demo player is not accepting commands.".to_string())?;
    }
    Ok(())
}

/// Resize the stage to a new region/aspect (window/layout resize) and tell the
/// engine to re-create its render window at the new size via `vid_restart`.
#[tauri::command]
pub async fn demo_player_set_region(
    app: AppHandle,
    state: State<'_, AppState>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    aspect: f32,
) -> Result<(), String> {
    let (parent, stage, tx) = {
        let guard = state.demo_player.inner.lock().unwrap();
        match guard.as_ref() {
            Some(s) => (s.parent, s.stage, s.cmd_tx.clone()),
            None => return Ok(()),
        }
    };
    let (sx, sy, sw, sh) = letterbox(x, y, w, h, aspect);
    reposition_stage_on_main(&app, parent, stage, sx, sy, sw, sh);
    let _ = tx.send("vid_restart".to_string());
    Ok(())
}

/// Reposition the stage to a new region/aspect WITHOUT a `vid_restart`. Used on
/// window MOVE (the launcher's client rect is unchanged, only its screen
/// position moved, so the owned popup must follow but the engine needn't
/// re-init). Cheap enough to call on every move event.
#[tauri::command]
pub async fn demo_player_reposition(
    app: AppHandle,
    state: State<'_, AppState>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    aspect: f32,
) -> Result<(), String> {
    let (parent, stage) = {
        let guard = state.demo_player.inner.lock().unwrap();
        match guard.as_ref() {
            Some(s) => (s.parent, s.stage),
            None => return Ok(()),
        }
    };
    let (sx, sy, sw, sh) = letterbox(x, y, w, h, aspect);
    reposition_stage_on_main(&app, parent, stage, sx, sy, sw, sh);
    Ok(())
}

/// Stop playback: kill the engine, close the control channel, destroy the stage.
#[tauri::command]
pub async fn demo_player_stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let session = {
        let mut guard = state.demo_player.inner.lock().unwrap();
        guard.take()
    };
    if let Some(s) = session {
        stop_session(&app, s);
    }
    Ok(())
}

/// Synchronously stop any active session. Safe to call from the main thread
/// (e.g. the window-close handler): unlike `demo_player_stop` it's not an async
/// command, so it can run in the `on_window_event` callback. Without this, hiding
/// the launcher to the tray would leave the spawned engine process running with
/// no UI to control it. Emits `demo-player-closed` so a still-mounted frontend
/// resets its playing state.
pub fn stop_active_session(app: &AppHandle) {
    let session = {
        let state = app.state::<AppState>();
        let mut guard = state.demo_player.inner.lock().unwrap();
        guard.take()
    };
    if let Some(s) = session {
        stop_session(app, s);
        app.emit("demo-player-closed", ()).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_pillarboxes_wide_region() {
        // 1000x400 region, 4:3 demo -> limited by height (400), width 533, centered.
        let (x, y, w, h) = letterbox(0, 0, 1000, 400, 4.0 / 3.0);
        assert_eq!(h, 400);
        assert_eq!(w, 533);
        assert_eq!(y, 0);
        assert_eq!(x, (1000 - 533) / 2);
    }

    #[test]
    fn letterbox_letterboxes_tall_region() {
        // 800x1000 region, 16:9 demo -> limited by width (800), height 450, centered.
        let (x, y, w, h) = letterbox(0, 0, 800, 1000, 16.0 / 9.0);
        assert_eq!(w, 800);
        assert_eq!(h, 450);
        assert_eq!(x, 0);
        assert_eq!(y, (1000 - 450) / 2);
    }

    #[test]
    fn letterbox_offset_region_translates() {
        let (x, y, _, _) = letterbox(100, 50, 800, 450, 16.0 / 9.0);
        // exact aspect match -> fills region, origin preserved
        assert_eq!((x, y), (100, 50));
    }

    #[test]
    fn parse_status_reads_all_fields() {
        let ev = parse_status("status time 1500 start 1000 total 4000 demo 1 paused 0 atend 0")
            .unwrap();
        assert_eq!(ev.time, 1500);
        assert_eq!(ev.start, 1000);
        assert_eq!(ev.total, 4000);
        assert!(ev.demo);
        assert!(!ev.paused);
        assert!(!ev.atend);
    }

    #[test]
    fn parse_status_rejects_other_lines() {
        assert!(parse_status("hello world").is_none());
        assert!(parse_status("").is_none());
    }

    #[test]
    fn derive_demo_launch_standard_layout() {
        let (base, game, arg) =
            derive_demo_launch(std::path::Path::new("/q3/defrag/demos/map[df].dm_68")).unwrap();
        assert_eq!(base, std::path::PathBuf::from("/q3"));
        assert_eq!(game, "defrag");
        assert_eq!(arg, "map[df].dm_68");
    }

    #[test]
    fn derive_demo_launch_subfolder() {
        let (base, game, arg) = derive_demo_launch(std::path::Path::new(
            "/q3/defrag/demos/sub/dir/run.dm_68",
        ))
        .unwrap();
        assert_eq!(base, std::path::PathBuf::from("/q3"));
        assert_eq!(game, "defrag");
        assert_eq!(arg, "sub/dir/run.dm_68");
    }

    #[test]
    fn derive_demo_launch_rejects_non_demos() {
        assert!(derive_demo_launch(std::path::Path::new("/q3/defrag/foo.dm_68")).is_err());
    }
}
