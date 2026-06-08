//! Embedded demo player (Windows + Linux).
//!
//! Plays a Quake3 `.dm_68` Defrag demo *inside* the launcher window. We ship a
//! purpose-built oDFe engine build (resources/odfe/) that can render into a
//! launcher-supplied child window and be driven over a loopback control
//! channel - see the engine's `code/win32/win_glimp.c` (`in_embedParent`, Win32)
//! and `code/sdl/sdl_embed.c` (X11 reparenting, Linux) plus
//! `code/client/cl_control.c` (`in_controlPort`).
//!
//! Platform note: on Windows the "stage" is an owned WS_POPUP window over the
//! WebView2; on Linux it's an X11 child window of the launcher's GTK toplevel
//! that the engine reparents into (works on X11 and, via XWayland, on Wayland -
//! we force the launcher onto the X11 GDK backend at startup). macOS is not
//! supported (no embed path) and returns an error.
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

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::AppState;

/// One running engine instance ("pane"). A normal playback is a single pane;
/// the side-by-side demo comparison runs two (pane 0 = left, pane 1 = right),
/// driven in lockstep. Everything that used to be "the session" is now a pane,
/// and the player holds a `Vec<Pane>` (0, 1 or 2 entries).
struct Pane {
    /// Which half this pane occupies: 0 = sole/left, 1 = right.
    index: u8,
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
    /// Per-pane seek offset (ms), default 0. A synchronized relative seek of `t`
    /// lands this pane at `t + offset`. Used in comparison to nudge one demo
    /// against the other so the runs line up (the engine reports only the demo-
    /// FILE start, not the defrag timer start, so the user fine-tunes the sync).
    offset: Arc<AtomicI32>,
    join: Option<JoinHandle<()>>,
}

/// App-state holder for the active demo player. Empty = nothing playing; one
/// pane = normal playback; two panes = side-by-side comparison.
#[derive(Default)]
pub struct DemoPlayer {
    inner: Mutex<Vec<Pane>>,
}

/// Status snapshot pushed to the frontend ~10x/sec while a demo plays.
/// Mirrors the engine's `status time <t> start <s> total <T> demo <d> paused <p>
/// atend <a>` line. The playhead position is `time - start`; the demo length is
/// `total - start` (total is 0 until the launcher seeks once to measure it).
#[derive(Clone, serde::Serialize)]
struct StatusEvent {
    /// Which pane this status is from: 0 = sole/left, 1 = right (comparison).
    pane: u8,
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

/// Grid dimensions (cols, rows) for `count` comparison panes: 1 = 1x1, 2 = 2x1,
/// 3 = 3x1 (a row of three), 4 = 2x2.
fn grid_dims(count: u8) -> (u8, u8) {
    match count {
        0 | 1 => (1, 1),
        2 => (2, 1),
        3 => (3, 1),
        _ => (2, 2),
    }
}

/// Split a render region into the sub-region for `pane` of `count` panes laid
/// out in a grid (see `grid_dims`), leaving a thin gutter between cells so the
/// demos read as distinct panels. For count == 1 it returns the whole region.
fn pane_region(pane: u8, count: u8, rx: i32, ry: i32, rw: i32, rh: i32) -> (i32, i32, i32, i32) {
    if count <= 1 {
        return (rx, ry, rw, rh);
    }
    const GUTTER: i32 = 6; // px between cells
    // Each engine is inset inside its cell so the launcher can paint a coloured
    // identification border in the margin (the webview shows through there). The
    // frontend draws the border at the even cell boundary; INSET keeps the
    // engine from covering it.
    const INSET: i32 = 4;
    let (cols, rows) = grid_dims(count);
    let col = (pane % cols) as i32;
    let row = (pane / cols) as i32;
    let cols = cols as i32;
    let rows = rows as i32;

    // Cell width/height after removing the gutters between cells.
    let cw = (rw - GUTTER * (cols - 1)) / cols;
    let ch = (rh - GUTTER * (rows - 1)) / rows;
    let cx = rx + col * (cw + GUTTER);
    let cy = ry + row * (ch + GUTTER);
    // Last column/row absorbs rounding remainder so panes meet the region edge.
    let w = if col == cols - 1 { (rx + rw - cx).max(1) } else { cw.max(1) };
    let h = if row == rows - 1 { (ry + rh - cy).max(1) } else { ch.max(1) };
    (cx + INSET, cy + INSET, (w - 2 * INSET).max(1), (h - 2 * INSET).max(1))
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
        HWND_TOP, SWP_NOACTIVATE, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_NOACTIVATE,
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

// Linux stage: a raw X11 child window of the launcher's GTK toplevel. The engine
// (a separate process) reparents its own X11 window into this one - X11 allows
// reparenting into a window owned by another client given its id. Because it's a
// child of the toplevel, it follows the launcher automatically when the window
// moves (no per-move reposition needed, unlike the Windows owned popup); we still
// move/resize it on layout changes. All calls run on the main (GTK) thread.
#[cfg(target_os = "linux")]
mod stage {
    use std::os::raw::{c_int, c_uint, c_ulong};
    use std::sync::Mutex;
    use x11::xlib;

    // One shared Xlib connection for every stage window, opened lazily. A Display*
    // isn't Send, so we stash it as a usize behind a mutex; it's only ever touched
    // on the main thread (all stage helpers dispatch there), and kept open for the
    // process lifetime so the windows it owns survive.
    static DISPLAY: Mutex<usize> = Mutex::new(0);

    fn display() -> *mut xlib::Display {
        let mut g = DISPLAY.lock().unwrap();
        if *g == 0 {
            let d = unsafe { xlib::XOpenDisplay(std::ptr::null()) };
            *g = d as usize;
        }
        *g as *mut xlib::Display
    }

    /// Create a black child window of `owner` (the launcher toplevel's X11 window)
    /// at owner-relative (cx,cy,w,h). The engine reparents its render window into
    /// this. Returns the new window id, or 0 on failure.
    pub fn create(owner: isize, cx: i32, cy: i32, w: i32, h: i32) -> isize {
        if owner == 0 {
            return 0;
        }
        let dpy = display();
        if dpy.is_null() {
            return 0;
        }
        unsafe {
            let screen = xlib::XDefaultScreen(dpy);
            let black = xlib::XBlackPixel(dpy, screen);
            let win = xlib::XCreateSimpleWindow(
                dpy,
                owner as c_ulong,
                cx as c_int,
                cy as c_int,
                w.max(1) as c_uint,
                h.max(1) as c_uint,
                0,
                black,
                black,
            );
            if win == 0 {
                return 0;
            }
            xlib::XMapWindow(dpy, win);
            xlib::XRaiseWindow(dpy, win);
            xlib::XSync(dpy, 0);
            win as isize
        }
    }

    /// Move/resize the stage to a new owner-relative rect and keep it on top.
    /// `_owner` is unused (the window remembers its parent); kept for signature
    /// parity with the Windows backend.
    pub fn reposition(_owner: isize, win: isize, cx: i32, cy: i32, w: i32, h: i32) {
        if win == 0 {
            return;
        }
        let dpy = display();
        if dpy.is_null() {
            return;
        }
        unsafe {
            xlib::XMoveResizeWindow(
                dpy,
                win as c_ulong,
                cx as c_int,
                cy as c_int,
                w.max(1) as c_uint,
                h.max(1) as c_uint,
            );
            xlib::XRaiseWindow(dpy, win as c_ulong);
            xlib::XSync(dpy, 0);
        }
    }

    pub fn destroy(win: isize) {
        if win == 0 {
            return;
        }
        let dpy = display();
        if dpy.is_null() {
            return;
        }
        unsafe {
            xlib::XDestroyWindow(dpy, win as c_ulong);
            xlib::XSync(dpy, 0);
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
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

// ---- transport-key hook ----------------------------------------------------
//
// The embedded engine renders into a child of our stage window, but it is a
// SEPARATE PROCESS. When the user clicks the demo, keyboard focus moves to the
// engine's window, so the launcher's WebView never sees keydowns and its
// transport shortcuts go dead until the user clicks back into the launcher.
// Forwarding keys from inside the engine only works while the engine window
// actually holds focus, which in this cross-process embed it often doesn't.
//
// A low-level keyboard hook (WH_KEYBOARD_LL) sees every keystroke system-wide,
// before any window, regardless of which process has focus. We install it while
// a demo plays and act ONLY when the demo is the focused context (foreground is
// our stage window, or a window belonging to the engine process) - so we never
// hijack keys while the user types in the launcher's own UI (there the WebView's
// own handler runs). Matching keys are turned into `demo-player-key` events and
// swallowed so nothing else also reacts.

/// Context the hook needs: where to emit, and how to recognize "demo focused".
/// `panes` is one `(stage hwnd, engine pid)` per running pane, so a key fires
/// when EITHER pane (in comparison mode) holds focus.
#[cfg(windows)]
struct KeyHookCtx {
    app: AppHandle,
    panes: Vec<(isize, u32)>,
}

#[cfg(windows)]
static KEY_HOOK_CTX: Mutex<Option<KeyHookCtx>> = Mutex::new(None);
/// Installed HHOOK as isize (0 = none). Stored so we can unhook on stop.
#[cfg(windows)]
static KEY_HOOK_HANDLE: Mutex<isize> = Mutex::new(0);

#[cfg(windows)]
unsafe extern "system" fn ll_keyboard_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, HC_ACTION, KBDLLHOOKSTRUCT,
    };

    const WM_KEYDOWN: u32 = 0x0100;
    const WM_SYSKEYDOWN: u32 = 0x0104;

    if code == HC_ACTION as i32 {
        let msg = wparam.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            // map the few transport keys we care about (VK_* codes)
            let name = match kb.vkCode {
                0x1B => Some("esc"),
                0x20 => Some("space"),
                0x25 => Some("left"),
                0x26 => Some("up"),
                0x27 => Some("right"),
                0x28 => Some("down"),
                _ => None,
            };
            if let Some(name) = name {
                if let Ok(guard) = KEY_HOOK_CTX.lock() {
                    if let Some(ctx) = guard.as_ref() {
                        // Is ANY of our demo panes the focused context?
                        let fg = GetForegroundWindow();
                        let fg_isize = fg.0 as isize;
                        let mut fg_pid = 0u32;
                        GetWindowThreadProcessId(fg, Some(&mut fg_pid));
                        let in_demo = ctx.panes.iter().any(|(stage, pid)| {
                            fg_isize == *stage || (fg_pid != 0 && fg_pid == *pid)
                        });
                        if in_demo {
                            ctx.app.emit("demo-player-key", name.to_string()).ok();
                            return LRESULT(1); // swallow - nothing else reacts
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Start intercepting transport keys for the active panes (one `(stage, pid)`
/// per pane). Installs the system hook once; safe to call after all panes exist.
fn install_key_hook(app: &AppHandle, panes: Vec<(isize, u32)>) {
    #[cfg(windows)]
    {
        *KEY_HOOK_CTX.lock().unwrap() = Some(KeyHookCtx {
            app: app.clone(),
            panes,
        });
        // LL hooks fire on the installing thread's message loop - install on the
        // main (UI) thread, which Tauri pumps.
        let _ = app.run_on_main_thread(|| unsafe {
            use windows::Win32::System::LibraryLoader::GetModuleHandleW;
            use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_KEYBOARD_LL};
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            if let Ok(h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard_proc), Some(hinst.into()), 0) {
                *KEY_HOOK_HANDLE.lock().unwrap() = h.0 as isize;
            }
        });
    }
    #[cfg(not(windows))]
    {
        let _ = (app, panes);
    }
}

/// Stop intercepting transport keys.
fn remove_key_hook(app: &AppHandle) {
    #[cfg(windows)]
    {
        let h = {
            let mut g = KEY_HOOK_HANDLE.lock().unwrap();
            let v = *g;
            *g = 0;
            v
        };
        if h != 0 {
            let _ = app.run_on_main_thread(move || unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{UnhookWindowsHookEx, HHOOK};
                let _ = UnhookWindowsHookEx(HHOOK(h as *mut std::ffi::c_void));
            });
        }
        *KEY_HOOK_CTX.lock().unwrap() = None;
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
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
        pane: 0, // filled in by the caller (run_control knows its pane)
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
    pane: u8,
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
                        let line = line.trim_end();
                        if let Some(mut ev) = parse_status(line) {
                            ev.pane = pane;
                            app.emit("demo-player-status", ev).ok();
                        } else if let Some(key) = line.strip_prefix("key ") {
                            // The engine forwards a transport key it swallowed
                            // while its render window held focus; surface it so
                            // the frontend runs the matching shortcut.
                            app.emit("demo-player-key", key.trim().to_string()).ok();
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

/// Tear down a single pane: signal its thread, kill the engine, drop its stage
/// window. Does NOT touch the shared key hook (see `stop_all`).
fn stop_pane(app: &AppHandle, mut p: Pane) {
    p.stop.store(true, Ordering::Relaxed);
    if let Ok(mut c) = p.child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    if let Some(j) = p.join.take() {
        let _ = j.join();
    }
    destroy_stage_on_main(app, p.stage);
}

/// Tear down every active pane (single playback or a comparison pair) and remove
/// the shared transport-key hook once.
fn stop_all(app: &AppHandle, panes: Vec<Pane>) {
    if panes.is_empty() {
        return;
    }
    remove_key_hook(app);
    for p in panes {
        stop_pane(app, p);
    }
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

// ---- pane spawning ---------------------------------------------------------

/// Resolve the bundled engine binary and its directory (shared by single +
/// compare). The filename is platform-specific (.exe on Windows, no extension on
/// Linux); both ship in resources/odfe/.
#[cfg(any(windows, target_os = "linux"))]
fn resolve_engine(app: &AppHandle) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    #[cfg(windows)]
    const ENGINE_REL: &str = "resources/odfe/oDFe.x64.exe";
    #[cfg(target_os = "linux")]
    const ENGINE_REL: &str = "resources/odfe/oDFe.x64";

    let exe = app
        .path()
        .resolve(ENGINE_REL, tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("Could not locate the bundled demo-player engine: {e}"))?;
    if !exe.exists() {
        return Err(format!("Bundled demo-player engine missing at {}", exe.display()));
    }
    let exe_dir = exe
        .parent()
        .ok_or_else(|| "Bundled engine path has no parent.".to_string())?
        .to_path_buf();
    Ok((exe, exe_dir))
}

/// Resolve the launcher main window's native parent handle for the stage: the
/// HWND on Windows, the toplevel X11 window id on Linux. Returns a user-facing
/// error (surfaced in the UI) when embedding isn't possible - most importantly on
/// a Wayland session with no X11 window to embed into, where we tell the user how
/// to get an X11 session instead of just showing a black area.
#[cfg(any(windows, target_os = "linux"))]
fn main_window_handle(app: &AppHandle) -> Result<isize, String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window not available.".to_string())?;
    #[cfg(windows)]
    {
        Ok(win.hwnd().map_err(|e| e.to_string())?.0 as isize)
    }
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = win.window_handle().map_err(|e| e.to_string())?;
        match handle.as_raw() {
            RawWindowHandle::Xlib(x) => Ok(x.window as isize),
            RawWindowHandle::Xcb(x) => Ok(x.window.get() as isize),
            _ => Err(
                "The demo player needs an X11 session. Your desktop seems to be \
                 running native Wayland, which can't embed the player. Restart the \
                 launcher with GDK_BACKEND=x11, or log into an \"Xorg\"/\"X11\" \
                 session, and try again."
                    .to_string(),
            ),
        }
    }
}

/// Spawn one embedded engine pane: create its stage at the aspect-correct
/// sub-rect of the outer region `(rx,ry,rw,rh)`, launch the engine into it over
/// a fresh control port, and wire up the control thread. Does NOT install the
/// key hook - the caller does that once after every pane exists. `homepath`, if
/// set, isolates this instance's writable config dir so two engines comparing
/// side by side don't clobber each other's `q3config.cfg`.
#[cfg(any(windows, target_os = "linux"))]
#[allow(clippy::too_many_arguments)]
fn spawn_pane(
    app: &AppHandle,
    exe: &std::path::Path,
    exe_dir: &std::path::Path,
    demo_abs: &str,
    index: u8,
    parent: isize,
    rx: i32,
    ry: i32,
    rw: i32,
    rh: i32,
    aspect: f32,
    homepath: Option<std::path::PathBuf>,
) -> Result<Pane, String> {
    let (basepath, fs_game, demo_arg) = derive_demo_launch(std::path::Path::new(demo_abs))?;

    let (sx, sy, sw, sh) = letterbox(rx, ry, rw, rh, aspect);
    let stage = create_stage_on_main(app, parent, sx, sy, sw, sh);
    if stage == 0 {
        return Err("Failed to create the demo render window.".to_string());
    }

    let port = pick_port();
    let mut cmd = std::process::Command::new(exe);
    cmd.current_dir(exe_dir)
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
        // The embedded engine windows are never the OS-foreground window (the
        // launcher is), so the engine treats them as "unfocused" and would cap
        // at com_maxfpsUnfocused (default 60). Force both the focused and
        // unfocused caps to 125 (8 ms/frame) so every pane renders smoothly
        // regardless of which window has focus - this is the deliberate bypass.
        .arg("+set")
        .arg("com_maxfps")
        .arg("125")
        .arg("+set")
        .arg("com_maxfpsUnfocused")
        .arg("125")
        .arg("+set")
        .arg("fs_basepath")
        .arg(basepath.to_string_lossy().to_string());
    if let Some(hp) = &homepath {
        std::fs::create_dir_all(hp).ok();
        cmd.arg("+set").arg("fs_homepath").arg(hp.to_string_lossy().to_string());
    }
    cmd.arg("+set")
        .arg("fs_game")
        .arg(&fs_game)
        .arg("+demo")
        .arg(&demo_arg);

    // Linux: force SDL onto the X11 video driver so the engine window is a real
    // X11 window we can reparent (under Wayland SDL would otherwise make a
    // Wayland surface, which can't be embedded - it runs through XWayland this
    // way). in_nograb keeps the engine from grabbing the pointer so the
    // launcher's transport UI around the demo stays clickable.
    #[cfg(target_os = "linux")]
    {
        cmd.env("SDL_VIDEODRIVER", "x11");
        cmd.arg("+set").arg("in_nograb").arg("1");
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            destroy_stage_on_main(app, stage);
            return Err(format!("Failed to launch the demo-player engine: {e}"));
        }
    };

    let child = Arc::new(Mutex::new(child));
    let stop = Arc::new(AtomicBool::new(false));
    let offset = Arc::new(AtomicI32::new(0));
    let (cmd_tx, cmd_rx) = mpsc::channel::<String>();

    let join = {
        let app = app.clone();
        let child = child.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name(format!("demo-player-control-{index}"))
            .spawn(move || run_control(app, index, port, child, cmd_rx, stop))
            .ok()
    };

    Ok(Pane {
        index,
        child,
        stop,
        cmd_tx,
        stage,
        parent,
        offset,
        join,
    })
}

/// The `(stage, engine pid)` of a pane, for the focus check in the key hook.
#[cfg(windows)]
fn pane_focus_key(p: &Pane) -> (isize, u32) {
    let pid = p.child.lock().map(|c| c.id()).unwrap_or(0);
    (p.stage, pid)
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
#[cfg_attr(not(any(windows, target_os = "linux")), allow(unused_variables))]
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
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err("The embedded demo player isn't available on this platform.".to_string())
    }
    #[cfg(any(windows, target_os = "linux"))]
    {
        if demo.trim().is_empty() {
            return Err("No demo selected.".to_string());
        }

        let (exe, exe_dir) = resolve_engine(&app)?;

        // Parent the stage to the launcher's main window (HWND / X11 toplevel).
        let parent: isize = main_window_handle(&app)?;

        // Replace any existing session before creating the new one.
        {
            let panes: Vec<Pane> = state.demo_player.inner.lock().unwrap().drain(..).collect();
            stop_all(&app, panes);
        }

        let pane = spawn_pane(&app, &exe, &exe_dir, &demo, 0, parent, x, y, w, h, aspect, None)?;
        // Windows needs a low-level key hook to catch transport keys regardless
        // of focus; on Linux the engine forwards them up the control channel
        // (CL_Control_SendKey) instead, so there's nothing to install.
        #[cfg(windows)]
        install_key_hook(&app, vec![pane_focus_key(&pane)]);

        let mut guard = state.demo_player.inner.lock().unwrap();
        guard.push(pane);
        // The frontend reads playback via status events; the return value is
        // just a started-pane count kept for command-signature compatibility.
        Ok(1)
    }
}

/// Start a comparison of 2-4 demos (premium/token-gated in the UI). Each demo
/// plays in its own engine, tiled into a grid (see `pane_region`), all driven in
/// lockstep by the shared transport. Per-pane sync offsets (`demo_player_set_offset`)
/// let the user line runs up.
#[tauri::command]
#[cfg_attr(not(any(windows, target_os = "linux")), allow(unused_variables))]
pub async fn demo_player_compare_start(
    app: AppHandle,
    state: State<'_, AppState>,
    demos: Vec<String>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    aspect: f32,
) -> Result<u16, String> {
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err("The embedded demo player isn't available on this platform.".to_string())
    }
    #[cfg(any(windows, target_os = "linux"))]
    {
        let demos: Vec<String> = demos.into_iter().filter(|d| !d.trim().is_empty()).collect();
        if demos.len() < 2 {
            return Err("Pick at least two demos to compare.".to_string());
        }
        if demos.len() > 4 {
            return Err("You can compare at most four demos at once.".to_string());
        }
        let count = demos.len() as u8;

        let (exe, exe_dir) = resolve_engine(&app)?;
        let parent: isize = main_window_handle(&app)?;

        // Replace whatever is playing.
        {
            let panes: Vec<Pane> = state.demo_player.inner.lock().unwrap().drain(..).collect();
            stop_all(&app, panes);
        }

        let tmp = std::env::temp_dir();
        let mut started: Vec<Pane> = Vec::with_capacity(demos.len());
        for (i, demo) in demos.iter().enumerate() {
            let idx = i as u8;
            let (cx, cy, cw, ch) = pane_region(idx, count, x, y, w, h);
            // Isolate each engine's writable config so they don't fight over
            // q3config.cfg on quit.
            let homepath = tmp.join(format!("drl-demo-pane{idx}"));
            match spawn_pane(
                &app, &exe, &exe_dir, demo, idx, parent, cx, cy, cw, ch, aspect, Some(homepath),
            ) {
                Ok(p) => started.push(p),
                Err(e) => {
                    // Roll back any panes already started so we never leave a
                    // half-started comparison.
                    for p in started.drain(..) {
                        stop_pane(&app, p);
                    }
                    return Err(e);
                }
            }
        }

        // Windows: hook transport keys system-wide. Linux: engine forwards them.
        #[cfg(windows)]
        install_key_hook(&app, started.iter().map(pane_focus_key).collect());

        let mut guard = state.demo_player.inner.lock().unwrap();
        for p in started {
            guard.push(p);
        }
        Ok(count as u16)
    }
}

/// Send a verbatim console line to EVERY running pane (transport control:
/// `timescale 0.5`, `demopause 1`, ...). In comparison mode this fans the same
/// command out to both engines so they stay in lockstep. No-op (Ok) if nothing
/// is playing. NOTE: absolute `seekdemo` should NOT go through here in
/// comparison mode - use `demo_player_seek_relative`, which aligns per pane.
#[tauri::command]
pub fn demo_player_command(state: State<'_, AppState>, line: String) -> Result<(), String> {
    let guard = state.demo_player.inner.lock().unwrap();
    for p in guard.iter() {
        // Ignore individual send errors (a pane whose engine just died); the
        // control thread will surface the close.
        let _ = p.cmd_tx.send(line.clone());
    }
    Ok(())
}

/// Seek every pane to the same playhead `ms` (0 = each demo's start), applying
/// that pane's sync offset. With both offsets 0 this just lands both engines at
/// the same position; nudging a pane's offset (see `demo_player_set_offset`)
/// shifts it so two runs recorded with different lead-ins line up. `seekdemo` is
/// measured from the demo's first frame, so a single pane (offset 0) behaves
/// exactly like a plain absolute seek.
#[tauri::command]
pub fn demo_player_seek_relative(state: State<'_, AppState>, ms: i32) -> Result<(), String> {
    let guard = state.demo_player.inner.lock().unwrap();
    for p in guard.iter() {
        let target = ms + p.offset.load(Ordering::Relaxed);
        let _ = p.cmd_tx.send(format!("seekdemo {}", target.max(0)));
    }
    Ok(())
}

/// Set a pane's sync offset (ms) for comparison alignment. The next synchronized
/// seek lands that pane at `playhead + offset`. Out-of-range pane indices are
/// ignored. Returns Ok always (best effort).
#[tauri::command]
pub fn demo_player_set_offset(state: State<'_, AppState>, pane: u8, ms: i32) -> Result<(), String> {
    let guard = state.demo_player.inner.lock().unwrap();
    if let Some(p) = guard.iter().find(|p| p.index == pane) {
        p.offset.store(ms, Ordering::Relaxed);
    }
    Ok(())
}

/// Seek ONE pane to playhead `ms` (its sync offset applied), leaving the other
/// pane untouched. Used when nudging demo B's alignment so demo A doesn't jump
/// back on every click. Unknown pane indices are ignored.
#[tauri::command]
pub fn demo_player_seek_pane(state: State<'_, AppState>, pane: u8, ms: i32) -> Result<(), String> {
    let guard = state.demo_player.inner.lock().unwrap();
    if let Some(p) = guard.iter().find(|p| p.index == pane) {
        let target = ms + p.offset.load(Ordering::Relaxed);
        let _ = p.cmd_tx.send(format!("seekdemo {}", target.max(0)));
    }
    Ok(())
}

/// Send a verbatim console line to ONE pane (e.g. `s_volume 0` to mute it during
/// a comparison so you only hear the demos you want). Unknown indices ignored.
#[tauri::command]
pub fn demo_player_pane_command(state: State<'_, AppState>, pane: u8, line: String) -> Result<(), String> {
    let guard = state.demo_player.inner.lock().unwrap();
    if let Some(p) = guard.iter().find(|p| p.index == pane) {
        let _ = p.cmd_tx.send(line);
    }
    Ok(())
}

/// Reposition every pane's stage for a new region/aspect. `restart` issues a
/// `vid_restart` per pane (needed when the client size changed); a plain move
/// (window dragged) passes false so the engine needn't re-init.
fn relayout(app: &AppHandle, state: &State<'_, AppState>, x: i32, y: i32, w: i32, h: i32, aspect: f32, restart: bool) {
    // Snapshot what each pane needs (index, parent, stage, tx) plus the total
    // count, so we drop the lock before doing Win32 / channel work.
    let layout: Vec<(u8, isize, isize, Sender<String>)> = {
        let guard = state.demo_player.inner.lock().unwrap();
        guard
            .iter()
            .map(|p| (p.index, p.parent, p.stage, p.cmd_tx.clone()))
            .collect()
    };
    let count = layout.len() as u8;
    for (index, parent, stage, tx) in layout {
        let (rx, ry, rw, rh) = pane_region(index, count, x, y, w, h);
        let (sx, sy, sw, sh) = letterbox(rx, ry, rw, rh, aspect);
        reposition_stage_on_main(app, parent, stage, sx, sy, sw, sh);
        if restart {
            let _ = tx.send("vid_restart".to_string());
        }
    }
}

/// Resize the stages to a new region/aspect (window/layout resize) and tell each
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
    relayout(&app, &state, x, y, w, h, aspect, true);
    Ok(())
}

/// Reposition the stages to a new region/aspect WITHOUT a `vid_restart`. Used on
/// window MOVE (the launcher's client rect is unchanged, only its screen
/// position moved, so the owned popups must follow but the engines needn't
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
    relayout(&app, &state, x, y, w, h, aspect, false);
    Ok(())
}

/// Stop playback: kill every engine, close the control channels, destroy stages.
#[tauri::command]
pub async fn demo_player_stop(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let panes: Vec<Pane> = {
        let mut guard = state.demo_player.inner.lock().unwrap();
        guard.drain(..).collect()
    };
    stop_all(&app, panes);
    Ok(())
}

/// Synchronously stop any active session. Safe to call from the main thread
/// (e.g. the window-close handler): unlike `demo_player_stop` it's not an async
/// command, so it can run in the `on_window_event` callback. Without this, hiding
/// the launcher to the tray would leave the spawned engine process running with
/// no UI to control it. Emits `demo-player-closed` so a still-mounted frontend
/// resets its playing state.
pub fn stop_active_session(app: &AppHandle) {
    let panes: Vec<Pane> = {
        let state = app.state::<AppState>();
        let mut guard = state.demo_player.inner.lock().unwrap();
        guard.drain(..).collect()
    };
    if !panes.is_empty() {
        stop_all(app, panes);
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
    fn pane_region_single_is_whole() {
        assert_eq!(pane_region(0, 1, 10, 20, 800, 600), (10, 20, 800, 600));
    }

    #[test]
    fn pane_region_splits_two_with_gutter() {
        // 800 wide, 6px gutter -> each half (800-6)/2 = 397, then inset 4px/side.
        let (lx, ly, lw, lh) = pane_region(0, 2, 0, 0, 800, 600);
        assert_eq!((lx, ly, lw, lh), (4, 4, 389, 592));
        let (rx, ry, rw, rh) = pane_region(1, 2, 0, 0, 800, 600);
        // right cell starts at 403 (half + gutter), inset by 4.
        assert_eq!((rx, ry, rw, rh), (407, 4, 389, 592));
        // the two inset halves never overlap and stay within the region
        assert!(lx + lw <= rx);
        assert!(rx + rw <= 800);
    }

    #[test]
    fn grid_dims_layouts() {
        assert_eq!(grid_dims(1), (1, 1));
        assert_eq!(grid_dims(2), (2, 1));
        assert_eq!(grid_dims(3), (3, 1));
        assert_eq!(grid_dims(4), (2, 2));
    }

    #[test]
    fn pane_region_four_is_2x2_quadrants() {
        // 800x600, 6px gutters: cells ~397x297; quadrants tile, inset 4px/side.
        let tl = pane_region(0, 4, 0, 0, 800, 600);
        let tr = pane_region(1, 4, 0, 0, 800, 600);
        let bl = pane_region(2, 4, 0, 0, 800, 600);
        let br = pane_region(3, 4, 0, 0, 800, 600);
        assert_eq!(tl, (4, 4, 389, 289));
        assert_eq!(tr.0, 407); // right column starts past gutter + inset
        assert_eq!(tr.1, 4);
        assert_eq!(bl.0, 4);
        assert_eq!(bl.1, 307); // bottom row starts past gutter + inset
        // bottom-right reaches near the region's far corner (minus inset)
        assert_eq!(br.0 + br.2, 796);
        assert_eq!(br.1 + br.3, 596);
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
