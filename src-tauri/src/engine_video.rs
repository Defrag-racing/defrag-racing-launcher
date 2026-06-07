//! Engine video-resolution resolver for the embedded demo player.
//!
//! When we embed oDFe inside the launcher window, the engine renders to the
//! host window's client rect and *ignores* the config's `r_mode` /
//! `r_customwidth` / `r_customheight` (we force a child window that fills its
//! parent). So the launcher has to decide the render region's size/aspect
//! itself - and to match what the player normally sees (HUD layout, FOV,
//! defrag widescreen configs), it must reproduce the engine's own mode
//! selection from the user's config.
//!
//! This is a direct port of oDFe's `CL_GetModeInfo` (code/client/cl_main.c)
//! plus its `cl_vidModes` table. Given the parsed config cvars and the
//! desktop resolution, `resolve()` returns the width/height/aspect the engine
//! would have used; the launcher then sizes its host region to that aspect
//! (letterboxed in the player panel; the literal pixel count can be scaled
//! down to fit, only the aspect has to match).

use std::path::Path;

/// One predefined video mode (matches oDFe's `vidmode_t`).
struct VidMode {
    width: i32,
    height: i32,
    pixel_aspect: f32,
}

macro_rules! vm {
    ($w:expr, $h:expr) => {
        VidMode { width: $w, height: $h, pixel_aspect: 1.0 }
    };
}

/// oDFe `cl_vidModes` (code/client/cl_main.c). Index = `r_mode` when >= 0.
/// Keep in sync with the engine if its table ever changes.
const CL_VID_MODES: &[VidMode] = &[
    vm!(320, 240),   // 0
    vm!(400, 300),   // 1
    vm!(512, 384),   // 2
    vm!(640, 480),   // 3
    vm!(800, 600),   // 4
    vm!(960, 720),   // 5
    vm!(1024, 768),  // 6
    vm!(1152, 864),  // 7
    vm!(1280, 1024), // 8  (5:4)
    vm!(1600, 1200), // 9
    vm!(2048, 1536), // 10
    vm!(856, 480),   // 11 (wide)
    vm!(1280, 960),  // 12
    vm!(1280, 720),  // 13
    vm!(1280, 800),  // 14 (16:10)
    vm!(1366, 768),  // 15
    vm!(1440, 900),  // 16 (16:10)
    vm!(1600, 900),  // 17
    vm!(1680, 1050), // 18 (16:10)
    vm!(1920, 1080), // 19
    vm!(1920, 1200), // 20 (16:10)
    vm!(2560, 1080), // 21 (21:9)
    vm!(3440, 1440), // 22 (21:9)
    vm!(3840, 2160), // 23
    vm!(4096, 2160), // 24 (4K)
];

/// The video cvars we care about, after applying config exec order.
#[derive(Debug, Clone)]
pub struct VideoCvars {
    pub r_mode: i32,
    pub r_customwidth: i32,
    pub r_customheight: i32,
    pub r_custom_pixel_aspect: f32,
    pub r_fullscreen: bool,
    /// `r_modeFullscreen` string ("" when unset).
    pub r_mode_fullscreen: String,
}

impl Default for VideoCvars {
    fn default() -> Self {
        // Engine defaults (Quake3e/oDFe): r_mode -2 = desktop resolution.
        VideoCvars {
            r_mode: -2,
            r_customwidth: 1600,
            r_customheight: 1024,
            r_custom_pixel_aspect: 1.0,
            r_fullscreen: false,
            r_mode_fullscreen: String::new(),
        }
    }
}

/// Resolved render target the engine would have used.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct RenderTarget {
    pub width: i32,
    pub height: i32,
    /// `width / (height * pixelAspect)` - the aspect the host region must
    /// match so the view isn't stretched.
    pub aspect: f32,
}

/// Port of `CL_GetModeInfo`. `(dw, dh)` is the desktop resolution (used for
/// `r_mode -2`). Returns `None` only for an out-of-range mode index, matching
/// the engine's `qfalse` return.
pub fn resolve(cv: &VideoCvars, dw: i32, dh: i32) -> Option<RenderTarget> {
    let mut mode = cv.r_mode;

    // Dedicated fullscreen mode overrides r_mode when set.
    if cv.r_fullscreen && !cv.r_mode_fullscreen.is_empty() {
        mode = cv.r_mode_fullscreen.trim().parse::<i32>().unwrap_or(mode);
    }

    if mode < -2 || mode >= CL_VID_MODES.len() as i32 {
        return None;
    }

    // Unknown desktop resolution -> fall back to mode 3 (640x480), like the engine.
    if mode == -2 && (dw == 0 || dh == 0) {
        mode = 3;
    }

    let (width, height, pixel_aspect) = if mode == -2 {
        (dw, dh, cv.r_custom_pixel_aspect)
    } else if mode == -1 {
        (cv.r_customwidth, cv.r_customheight, cv.r_custom_pixel_aspect)
    } else {
        let vm = &CL_VID_MODES[mode as usize];
        (vm.width, vm.height, vm.pixel_aspect)
    };

    if width <= 0 || height <= 0 {
        return None;
    }

    let aspect = width as f32 / (height as f32 * pixel_aspect);
    Some(RenderTarget { width, height, aspect })
}

/// Parse the video cvars out of one or more config files, applied in order
/// (later files override earlier ones, and within a file the last `seta`
/// wins - mirroring how the engine execs `q3config.cfg` then `autoexec.cfg`).
/// Missing files are skipped. Unset cvars keep their engine defaults.
pub fn parse_configs(paths: &[&Path]) -> VideoCvars {
    let mut cv = VideoCvars::default();
    for p in paths {
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        apply_config_text(&mut cv, &text);
    }
    cv
}

/// Apply `set`/`seta`/`sets`/`setu` assignments from a config blob to `cv`.
/// Cvar names are matched case-insensitively (Quake3 cvars are).
fn apply_config_text(cv: &mut VideoCvars, text: &str) {
    for line in text.lines() {
        let line = line.trim();
        // Split into at most 3 tokens: <setcmd> <cvar> <value...>
        let mut it = line.split_whitespace();
        let Some(cmd) = it.next() else { continue };
        let cmd = cmd.to_ascii_lowercase();
        if !matches!(cmd.as_str(), "set" | "seta" | "sets" | "setu") {
            continue;
        }
        let Some(name) = it.next() else { continue };
        let name = name.to_ascii_lowercase();
        // Value = the rest of the line, stripped of surrounding quotes.
        let rest = it.collect::<Vec<_>>().join(" ");
        let value = rest.trim().trim_matches('"').trim();

        match name.as_str() {
            "r_mode" => {
                if let Ok(v) = value.parse() {
                    cv.r_mode = v;
                }
            }
            "r_customwidth" => {
                if let Ok(v) = value.parse() {
                    cv.r_customwidth = v;
                }
            }
            "r_customheight" => {
                if let Ok(v) = value.parse() {
                    cv.r_customheight = v;
                }
            }
            "r_custompixelaspect" => {
                if let Ok(v) = value.parse::<f32>() {
                    if v > 0.0 {
                        cv.r_custom_pixel_aspect = v;
                    }
                }
            }
            "r_fullscreen" => {
                cv.r_fullscreen = value != "0" && !value.is_empty();
            }
            "r_modefullscreen" => {
                cv.r_mode_fullscreen = value.to_string();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_mode_uses_desktop_res() {
        let cv = VideoCvars { r_mode: -2, ..Default::default() };
        let t = resolve(&cv, 3440, 1440).unwrap();
        assert_eq!((t.width, t.height), (3440, 1440));
        assert!((t.aspect - 3440.0 / 1440.0).abs() < 1e-4);
    }

    #[test]
    fn custom_mode_uses_custom_res() {
        // The user's real config: r_mode -1, 1600x1024.
        let cv = VideoCvars { r_mode: -1, r_customwidth: 1600, r_customheight: 1024, ..Default::default() };
        let t = resolve(&cv, 3440, 1440).unwrap();
        assert_eq!((t.width, t.height), (1600, 1024));
        assert!((t.aspect - 1600.0 / 1024.0).abs() < 1e-4);
    }

    #[test]
    fn predefined_mode_indexes_table() {
        let cv = VideoCvars { r_mode: 22, ..Default::default() }; // 3440x1440
        let t = resolve(&cv, 1920, 1080).unwrap();
        assert_eq!((t.width, t.height), (3440, 1440));
    }

    #[test]
    fn fullscreen_mode_override() {
        let cv = VideoCvars {
            r_mode: -1,
            r_fullscreen: true,
            r_mode_fullscreen: "19".into(), // 1920x1080
            ..Default::default()
        };
        let t = resolve(&cv, 3440, 1440).unwrap();
        assert_eq!((t.width, t.height), (1920, 1080));
    }

    #[test]
    fn pixel_aspect_affects_aspect_only() {
        let cv = VideoCvars { r_mode: 3, r_custom_pixel_aspect: 2.0, ..Default::default() };
        // mode 3 = 640x480 but pixel aspect comes from the table (1.0), not the cvar.
        let t = resolve(&cv, 0, 0).unwrap();
        assert!((t.aspect - 640.0 / 480.0).abs() < 1e-4);
    }

    #[test]
    fn unknown_desktop_falls_back() {
        let cv = VideoCvars { r_mode: -2, ..Default::default() };
        let t = resolve(&cv, 0, 0).unwrap(); // -> mode 3 = 640x480
        assert_eq!((t.width, t.height), (640, 480));
    }

    #[test]
    fn out_of_range_mode_is_none() {
        let cv = VideoCvars { r_mode: 999, ..Default::default() };
        assert!(resolve(&cv, 1920, 1080).is_none());
    }

    #[test]
    fn config_parse_last_wins_and_overrides() {
        let mut cv = VideoCvars::default();
        // q3config first
        apply_config_text(&mut cv, "seta r_mode \"-1\"\nseta r_customWidth \"1600\"\nseta r_customHeight \"1024\"\n");
        assert_eq!(cv.r_mode, -1);
        assert_eq!(cv.r_customwidth, 1600);
        // autoexec overrides to desktop mode
        apply_config_text(&mut cv, "seta r_mode \"-2\"\n");
        assert_eq!(cv.r_mode, -2);
    }

    #[test]
    fn config_parse_case_insensitive_and_quotes() {
        let mut cv = VideoCvars::default();
        apply_config_text(&mut cv, "SETA R_Mode \"21\"\nseta r_fullscreen \"1\"\n");
        assert_eq!(cv.r_mode, 21);
        assert!(cv.r_fullscreen);
    }
}
