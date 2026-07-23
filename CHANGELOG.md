# Changelog

All notable changes to the Defrag Racing Launcher.

The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## 0.1.46

- **Fixed: the demo engine could open as a separate window instead of inside the launcher (Linux).** On some desktops (seen on Zorin/GNOME) the engine's own attempt to embed itself into the player pane fails, its window stays floating on the desktop and the pane shows black. The launcher now detects this within a few seconds and adopts the engine window into the pane itself. The bundled Linux engine also prints a clearer diagnostic about why a self-embed failed, to make future reports easier to debug.

## 0.1.45

- **Fixed: demos played in the launcher loaded as vanilla Quake on Linux.** The embedded player derived all its game folders from the demo's location, which hid Linux installs whose Defrag mod lives under the default home path (`~/.q3a`). The engine then fell back to baseq3's game code and every demo opened on the "CLIENT/SERVER GAME MISMATCH" / CD-key screen instead of playing. The player now always shows the engine all three places that matter - the engine install, `~/.q3a`, and its own private folder (the demo is copied there first, so it plays from ANY folder on disk). (Windows was unaffected.)
- **Fixed: black demo window on multi-monitor Linux setups.** With more than one monitor, the engine's window could re-apply its old on-screen position inside the player pane and land entirely outside the visible area - the demo was audible but the pane stayed black. The launcher now pins the engine window to the pane and keeps it there, and prints a window-layout diagnostic to the terminal to make future embed reports easy to debug.
- **Fixed: the demo player could write into your real Quake config on Linux.** The engine's writable home now points at a launcher-private folder instead of `~/.q3a`, so nothing the embedded player does can ever touch your real `q3config.cfg` or other files there.
- **Fixed: "Force re-check" plus "Check & repair" clicked in quick succession could lock up the launcher.** Cache clearing no longer blocks the app while holding the demo watcher, and repeated rescan requests now collapse into one queued rescan instead of piling up.
- **Fixed: the demo player used stock Quake settings on Linux.** It only looked for your `q3config.cfg` under the engine folder, but Linux keeps everything under `~/.q3a`, so it found nothing and the demo played with default settings. It now finds your config under either the install or the home path (and in `baseq3` or your mod folder) and seeds the player's config from it, so demos use your real settings.
- **Your demos folder no longer has to live under the engine.** Onboarding required the demos folder to sit inside the engine's install dir, which is wrong on Linux (the engine is in a system path while demos live in `~/.q3a/<game>/demos`). It now just has to be a `…/<game>/demos` folder wherever you keep it.
- **Linux: downloaded maps go where the engine actually looks.** Auto-downloaded map pk3s now land in the home-path `baseq3` (`~/.q3a/baseq3`) - writable and on the engine's search path - instead of next to the (often read-only) engine binary.
- **Missing maps are downloaded automatically before a demo plays.** Watching or comparing a demo whose map you don't have used to drop you into a black/wrong screen. The launcher now checks for the map first - exactly like the Maps tab - and fetches its pk3 into `baseq3` if it's missing. The "Preparing map…" overlay shows a real progress bar (percent + MB while downloading, and a "Checking your installed maps…" note during the initial scan) instead of a bare spinner. If the download fails (e.g. the map isn't on the server) you get a clear popup telling you to install it manually and contact an admin, instead of a broken demo.
- **Volume control in the demo player.** The single demo viewer now has a mute button and a volume slider (it had neither). In a comparison every demo gets its own volume slider next to its mute toggle, so you can balance the runs instead of only muting them.
- **Fixed: Win+Shift+S (and other Windows-key shortcuts) work over the demo.** While a demo had focus the embedded engine was swallowing the Windows key, which broke the Win+Shift+S screenshot shortcut. The embedded player no longer blocks it.

## 0.1.44

- **Frame-accurate scrubbing + frame stepping.** The timeline now scrubs in milliseconds (snapped to Quake's 8 ms frame grid) instead of whole seconds, so even a short demo gives you hundreds of positions and lands right on a frame. **Shift + ←/→** steps exactly one frame at a time (it pauses first) for frame-by-frame analysis.
- **The demo player no longer touches your real Quake config.** When you watch or compare demos in the launcher, the bundled engine now reads and writes its own `defrag.launcher.cfg` (seeded from your `q3config.cfg` each time) instead of your real config - so the settings it needs to run embedded never leak into the config your normal game uses.
- **Scrubbing while a demo plays is smooth now.** Dragging the timeline pauses the engine for the duration of the drag and resumes on release, so the playhead can't desync or freeze (which it did on Linux). The picture still follows the handle as you drag.
- **Onboarding only accepts a real demos folder.** When you pick your demos folder it now has to be your engine's `demos` folder (or a subfolder of it); picking something outside the Defrag install - which silently broke playback and `defrag://` joins - is rejected with a clear message.
- **Clearer wording in the Demos tab.** The "Hashing" status is now "Checking" with a plain-language tooltip (it fingerprints the file to skip re-uploading runs already on defrag.racing), and the auto-backup explanation spells out what it does and why.

## 0.1.43

- **Playback speeds are centered**, with a small gap separating the slow-motion speeds (0.1x-0.75x) from the normal/fast ones (1x-8x) so the two groups read apart at a glance.

## 0.1.42

- **Zooming the scrub bar no longer flickers/resizes the demo.** Holding the scrub handle to zoom into milliseconds used to grow the transport bar (the longer millisecond timer wrapped onto a second line), which shrank the demo area and forced the engine to resize and refresh its window - then again when you released. The transport bar now keeps a constant height, and the "ms zoom" readout sits on its own fixed line, so zooming in and out is seamless.
- **Playback speeds moved to their own row.** The 0.1x-8x speed buttons took up most of the transport bar; they now live on a dedicated row, giving the scrub bar the full width underneath.

## 0.1.41

- **The demo player now works on Linux.** Watching demos in the launcher and the side-by-side comparison - previously Windows-only - now run on Linux too. The bundled engine renders right inside the launcher window just like on Windows: same transport bar, scrubbing, speeds, multi-demo grid and per-pane sync. It runs on X11 and, on Wayland desktops, through XWayland automatically (the launcher pins itself to the X11 backend on startup). If you're on a native-Wayland-only setup the launcher tells you how to switch to an X11 session instead of just showing a black area.
- Transport keyboard shortcuts (Esc / Space / arrows) work over the demo on Linux as well - the engine hands them back to the launcher, so they do the same thing whether the launcher UI or the demo has focus.

## 0.1.40

- **History auto-refreshes.** The History tab now reloads itself every 30 seconds while it's open and the launcher window is focused, so connections you make in-game show up without pressing Refresh. It stays quiet when the window is in the background or you're on another tab.
- **Clearing history now asks first.** The **Clear** button used to wipe your whole connection history immediately - the confirmation prompt didn't actually appear inside the launcher. It now arms an inline **Clear all / Cancel** confirmation, so a stray click can't nuke your history.

## 0.1.39

- **Demos always render at 125 fps in the player.** The embedded engine windows are never the OS-foreground window, so the engine had been capping them at the unfocused limit (60 fps) - playback (and every comparison pane) now runs at a full 125 fps regardless of which window has focus.
- **Wider, easier-to-read onboarding.** The setup card is roomier so the feature lists wrap onto fewer lines instead of cramming into a narrow column.
- **The token step now spells out everything a token unlocks** - the same full feature list used elsewhere - instead of a short 3-item summary that hid most of what you get.
- **The launcher no longer offers its own bundled demo-player engine as a defrag:// engine.** The engine shipped for the embedded player is internal-only; it's now filtered out of the engine picker on Windows, macOS and Linux, so you only ever pick your real Defrag install.
- **Compare up to four demos at once (premium).** Click **⚖ Compare** on a demo, add up to three more (same-map runs float to the top), and they play in their own engines tiled in a grid and locked together - one transport, scrub, pause and speed drive all of them, and keyboard shortcuts work over any panel. Each pane is framed in its own colour (matching the names and sync rows) so you can tell which is which, with a per-demo mute toggle (only demo A plays sound by default, so the audio isn't a pile-up). Each demo gets its own **sync row** to line it up against the first; nudges are in whole frames (multiples of 8 ms, since Quake runs at 125 fps) from 1 frame up to 1000, they accumulate as you click, and they move only that demo. Token-gated.
- **More playback speeds.** Added 0.2x, 0.3x, 0.4x and 0.75x to the demo player so there's finer control in the slow-motion range.
- **Millisecond scrubbing.** Hold the scrub handle still for a moment and the bar zooms into a narrow window around that point at 1 ms resolution, so you can land on an exact millisecond; release to drop back to the full timeline. Drag to a window edge and it slides further.
- **No more stray "Connection Interrupted" while a demo is paused.** The embedded player's engine occasionally flashed the centered "Connection Interrupted" text and the bottom-right phone-jack icon when paused; that false positive is now suppressed during launcher-driven playback (bundled engine updated).

## 0.1.36

- **Watch your demos right inside the launcher.** Every demo in the Demos tab now has a **▶ Play instantly in launcher** button that plays it embedded in the window - no separate game launch, no setup. It uses a bundled engine, so it just works on Windows. Full transport bar underneath: separate **Play** and **Pause**, speed buttons from **0.1x to 8x**, and a scrub bar that previews the frame live as you drag it.
- **Keyboard shortcuts while watching:** **Space** pause/resume, **←/→** seek 5s (hold to scrub), **↑/↓** seek 10s, **Esc** to close. They work whether the launcher UI or the demo picture has focus - click anywhere and the keys still respond.
- **End-of-demo is clearer.** When a run finishes it freezes on the last frame and shows as paused; hitting **Play** replays it from the start.
- **The launcher remembers its window size and position** across restarts, instead of always reopening at the default size.
- **Cleaner exit behaviour.** Leaving the Demos tab stops playback, and closing the launcher to the tray now also stops the demo engine - no more playback continuing in the background with no controls.
- Removed the separate **Player** tab - demos play directly from the Demos tab, so there's one obvious place to do it.
- Updates install more smoothly: the installer now closes a running demo engine first, so upgrading no longer fails with a "file in use" error.

## 0.1.35

- **Offline Maps now finds maps in the `defrag` folder too, not just `baseq3`.** The engine loads pk3s from both the base game folder and the active mod folder (defrag), so maps you have under `defrag\` - e.g. ones auto-downloaded in-game - were being missed by the Offline tab. It now scans both folders and de-duplicates by map name (the copy the engine would actually load wins). A pk3 with no `maps/*.bsp` inside (texture/sound/config packs) is ignored, as before.
- No change to "Run offline" downloads: maps still download into `baseq3`, which the engine always reads regardless of `fs_game` - so a one-click offline map downloads first, then launches, and is always found.

## 0.1.34

- **Offline Maps no longer hammers the disk.** On big libraries the tab used to re-open *every* pk3 in baseq3 on each visit to read its index, pinning the disk at 100% and freezing the app for seconds to minutes. Now the full scan runs only **once** and is cached to a manifest on disk; re-opening the tab just does a cheap metadata check (file size/date) and reuses the cached list unless baseq3 actually changed. The scan also runs off the UI thread, so it never freezes the window.
- The Offline tab is now **paginated** (24 maps per page) instead of loading the whole library at once - so thumbnails are only ever extracted for the page you're looking at, not for thousands of maps up front.
- "Reset launcher" in Settings now also clears the Offline Maps caches (the levelshot thumbnails and the scan manifest), so a reset truly starts from zero.

## 0.1.33

- Maps tab now has an **Offline (local)** sub-tab listing the maps you actually have installed in the engine's baseq3 folder - no token needed, works without internet. Map names come from each pk3's `maps/*.bsp`, and the thumbnail is the pk3's levelshot (TGA levelshots are converted, same as the website). Each card has VQ3 / CPM "Run" buttons that launch straight into the map (it's already installed, so no download).
- It's built to be light: listing only reads the pk3 directory (no unpacking), thumbnails are extracted lazily as cards scroll into view (just the one small levelshot, never the whole map), and every extracted thumbnail is cached on disk - so re-opening the tab or restarting reads the cache instead of re-reading the pk3s.

## 0.1.32

- Maps tab now shows each map's weapons, items and functions as small icons over the thumbnail (same icon set as the website). The icons are bundled with the launcher, so the grid doesn't fire a network request per icon. Needs the Maps tab, which is token-gated like before.
- Onboarding's feature list is updated to cover everything that's been added - run-any-map-offline, map history, the Maps icons, and Developer mode - so new users see the full picture. The same expanded list now also shows in the "skip the token" confirmation and on the final screen after skipping, so people who go token-less still see what the launcher does.

## 0.1.31

- Maps tab: the offline-run controls are clearer. The "Run offline" row now reads "▶ Click to run offline instantly" on its own line above the VQ3 / CPM buttons, so it's obvious what the buttons do without crowding the card.

## 0.1.30

- Maps tab "Run offline" now auto-installs the map first. Clicking VQ3 / CPM downloads the map's pk3 into the engine's `baseq3` folder if it isn't already there, then launches straight into it - no more "map not found" when you don't have it locally. The download is keyed by the map's original pk3 filename (not the map name), so maps that share a pk3 aren't downloaded twice, while the engine still loads the specific map you clicked. The button shows a spinner while it works.

## 0.1.29

- Maps tab: every map card now has **VQ3** and **CPM** "Run offline" buttons that launch the engine straight into that map in the chosen physics (e.g. `+vq3 cityrocket` / `+cpm cityrocket`). Disabled with a hint until an engine is set in Settings.
- History tab: each connection now records the **maps that played on the server while you were in the game**, shown in an expandable list under the row (map + physics + time). The launcher watches the engine process it started for the connect and polls that server's current map in the background - even while it sits in the tray - logging each rotation, then stops automatically when you close the game. Needs a launcher token (the server list is token-locked); without one the map history stays empty.

## 0.1.28

- Linux: fixed external links (map pages, profiles, YouTube renders, the token page, etc.) not opening. Same root cause as the engine-launch fix - when the launcher runs as an AppImage, the browser was being spawned with the AppImage's mangled library environment and failed to start. Links now open through a system opener (xdg-open, with fallbacks to gio and common browsers) launched with a clean, shell-equivalent environment. Windows and macOS are unchanged.

## 0.1.27

- New Developer mode toggle in Settings (off by default). When enabled it reveals two power-user launch options: a Custom launch arguments field whose flags get appended to the standard Quick launch, and named Launch profiles - each with its own arguments, as many as you want. Arguments are parsed like a command line, so quotes keep a spaced value (`"my mod"`) as one argument.
- Launch profiles show up as a dropdown next to Quick launch in the top bar: click the caret to launch any profile (e.g. "Fullscreen", "Mod X") with just its own arguments, or the main button for your standard Quick launch. The menu only appears in developer mode with at least one profile defined, so nothing changes for normal users.

## 0.1.26

- The "update available" banner now shows on every tab, not just Demos. It used to live only on the Demos page, so an update was invisible while you were on Servers, Records, Maps, History or Notifications. It's now app-level (with the same View changes / Install & restart actions and inline changelog) and appears wherever you are.
- Settings' "Check now" is self-contained. When a manual check finds an update, the View changes and Install & restart buttons - plus the full changelog - now appear right there under the button, instead of telling you to go back to the Dashboard.

## 0.1.25

- Linux: fixed the engine misbehaving (e.g. libcurl and other libraries failing) when launched from the launcher while it worked fine started by hand. When the launcher runs as an AppImage, its startup script prepends the bundled `$APPDIR` libraries onto `LD_LIBRARY_PATH` (and a few module-loader vars) so its own binary finds them - but every process it spawned inherited that and loaded the AppImage's bundled libcurl/glib instead of the system ones. The launcher now hands the engine a clean, shell-equivalent environment (stripping only the AppImage's own additions, leaving anything you set intact), so it loads system libraries exactly as it does when you run the binary yourself.

## 0.1.24

- Pasting a token now tells you right away if it's wrong. The launcher checks the token against your account the moment you paste it - in onboarding and in Settings - instead of saving it and letting it fail silently later (the classic "I made the wrong token", only discovered hours later as a cryptic rejection on the Servers tab). A bad paste, a revoked token, or the wrong token type each get a specific message, and both screens point you at the exact place to fix it: the **Launcher Tokens** block under defrag.racing > Settings > Security. A valid token shows the account name you just signed in as.
- The "token rejected" message is no longer misleading. A token that simply isn't a Launcher Token used to report "account is restricted from uploading demos" even when nothing was restricted - it now reads as a wrong-token-type / restricted-account error, which is what's actually happening.

## 0.1.23

- Start with system is now on by default after setup. The launcher is a background companion - the demo watcher and the `defrag://` join handler only do anything while it's running - so it now enables launch-at-login when you finish onboarding instead of waiting for you to find the Settings toggle. It still starts hidden in the tray, and the same toggle turns it off if you'd rather launch it by hand.
- Fixed: clicking a `defrag://` join link while the launcher was closed opened the launcher but never offered Connect. On a cold start the launch URL only arrives in the process arguments (the in-app event that the running launcher listens for doesn't fire), and the launcher wasn't reading it - so it booted to the dashboard and dropped the link. It now reads the cold-start URL on launch and shows the Connect prompt (or auto-connects, if you enabled that), exactly as it does when it's already running. A dedup guard makes sure a single click can't connect twice.

## 0.1.22

- Setup steps reordered: the engine and demos folder (the required base) now come before the account token, so you clear the must-haves first and only then reach the one optional step. The token used to come first, which made the mandatory part feel like an afterthought.
- Skipping the token is no longer a one-tap accident. The "Skip - defrag:// only" button now opens a confirmation that names every feature that stays disabled (auto-backup, YouTube renders, server browser, Records and Maps, notifications) and makes you actively choose "Skip anyway". The wizard's last screen still shows the same warning with a one-click way back.

## 0.1.21

- Setup wizard no longer lets you skip the engine and demos folder. They're required - without an engine even `defrag://` join links can't open, and without a demos folder there's nothing to back up. Only the account token stays optional (you can still go `defrag://`-only).
- Reset now actually works and warns you properly. It used to rely on a browser confirm() dialog that WebView2 could silently swallow (so Reset did nothing). It's now a typed confirmation - you type "yes" to wipe settings + token, and it drops you back into the setup wizard from step 1. Your demos (local and on the server) are never touched.
- Removed the separate "Re-run setup" button. Reset is the one way to redo setup, and the engine path, demos folder and token are all editable directly in Settings anyway.

## 0.1.20

- The "Token required" screens on Servers, Records and Maps now have a button that takes you straight to the token field in Settings and highlights it, instead of just telling you to go there.
- Onboarding token step is clearer: the "open the token page" box now obviously reads as a clickable button ("Click here to open the token page", "Open" + opens-in-browser hint), and the instructions spell out exactly where to look - the **Launcher Tokens** block under defrag.racing > Settings > Security. The Settings token field says the same.

## 0.1.19

- The "what a token unlocks" list is now consistent everywhere it shows (Dashboard banner, Settings, onboarding) and names every gated feature: auto-backup, YouTube renders, the server browser, and the Records and Maps tabs, plus notifications. It used to list a different subset in each place and omit Records/Maps. (The History tab works without a token - it's your local defrag:// join log - so it's intentionally not listed.)

## 0.1.18

- "Processed this session" now reaches your real demo count instead of stalling a few hundred short (e.g. stuck at 4942 of 5409). On launch the activity list is restored from the last session with its demos already marked backed-up; the counter was skipping those rows when the startup re-scan re-confirmed them, so it never caught up. It now counts every demo the re-scan confirms, exactly once, so the number settles at your library size.

## 0.1.17

- Updates no longer pop a Windows admin (UAC) prompt. The launcher now installs per-user (like Discord or VS Code) instead of into Program Files, so the auto-updater applies new versions silently in the background - no more elevation dialog that only blinked in the taskbar and wouldn't come to the foreground. One-time step for existing users: this update may install alongside the old version; if you see two "Defrag Racing Launcher" entries in Add/Remove Programs, uninstall the older one. The Windows download is now a single `.exe` (the `.msi` is gone).
- New "Check & repair" in Settings. Runs a quick scan of the launcher's local state - login/token, demos folder (exists + writable), backup cache and activity-list files, the watcher, and the engine path - and shows a green/amber/red line for each. Corrupt cache or queue files get a one-click Fix that safely resets them (your demos on the server are never touched). Handy for diagnosing a stuck install without digging through %APPDATA%.

## 0.1.16

- The session summary no longer claims demos were uploaded when they weren't. "X uploaded" now counts only real uploads this run; a demo re-confirmed from the local cache (already on the server from a previous run) correctly counts as "already backed up". Before, a demo that was only re-checked could be tallied as a fresh upload.
- The 30-minute safety rescan no longer looks like it reprocessed your whole library. It now skips demos already confirmed backed up this session instead of re-checking (and re-counting) every one, so "processed this session" reflects real work rather than doubling every rescan. No demos were ever actually re-hashed or re-uploaded - this was a counting issue - but the numbers now match reality.

## 0.1.15

- Fixed some demos showing a greyed-out, un-clickable Render button. A recently backed-up demo could appear twice - once as a real row and once as a hash-less ghost row - because the live queue and the on-disk list disagreed on the file's path form; both sides now use one normalised path.

## 0.1.14

- Really fixed the demo stuck on "Backing up 0/1". The 0.1.13 fix missed the common case: a demo that was already uploaded but then touched on disk (mtime changed) would refuse to clear and the launcher would keep re-checking it, pegging CPU the whole time. The launcher now reconciles the backup queue against its upload cache at launch, so an already-backed-up demo is recognised instantly instead of spinning forever. It also self-heals offline: if a file's contents match what was already uploaded, it's cleared without needing the server.
- Root cause behind it fixed too: on Windows the watcher and the cache could disagree on a file's path (verbatim "\\?\" prefix vs plain), keying the same demo into two buckets so it never matched its own backup record. Paths are now normalised everywhere they're compared.
- Big CPU drop with large demo folders. The activity list is now virtualised (only the rows you can see are rendered), live backup updates are coalesced to one repaint per frame instead of up to 20 per second, and the in-memory queue is bounded so a multi-thousand-demo library no longer ships its whole contents on every update.

## 0.1.13

- Demos tab no longer freezes for a few seconds on launch when you have a lot of rendered videos, and your YouTube render links now survive a restart instead of being re-fetched from scratch every time you open the launcher.
- Live backup progress: a strip now shows the demo being backed up right now (with speed), an X/Y count and a progress bar, so a slow CPU-throttled pass reads as "working" instead of looking frozen.
- Fixed a demo that could get stuck on "Backing up 0/1" at launch and never move until you hit Stop then Start.
- "Force re-check" no longer disables the Render buttons or drops the YouTube links on your demos, and it now has a short cooldown so it can't be fired off repeatedly by accident.

## 0.1.12

- Notifications layout: the per-row timestamp and the read/unread dot are now on the same line (right-aligned), so each notification fits more cleanly on one row instead of stacking.

## 0.1.11

- Release workflow rebuilt so multi-platform tag builds no longer race themselves. A `create-release` job now opens the draft once before any builder runs, the 4-platform matrix uploads into that draft by id, and a final `publish` job auto-flips it to published when every platform succeeds.
- No user-visible launcher changes vs 0.1.10 - this release exists so the auto-updater stops being stuck on a partial 0.1.10 draft.

## 0.1.10

- "What's new" panel on the Dashboard next to the update banner: expands to show every version's notes between your installed version and the latest release.
- Launcher pulls release notes from this CHANGELOG.md directly, so a user on an old version sees the full stack of changes since their build.
- (Release process bug: matrix race created duplicate drafts and the published draft was missing macOS Apple Silicon + Linux artifacts. Fixed in 0.1.11. This entry is kept for the historical trail; the actual user-facing payload first shipped intact in 0.1.11.)

## 0.1.9

- Bell-badge poll dropped from 90s to 180s and now skips ticks while the launcher window is hidden (tray mode), with an immediate refresh when you bring the window back.
- Notifications view no longer runs its own 60s poller - it subscribes to the shared store so the cadence is honest.
- Records pagination switched to Laravel's simplePaginate semantics (next/prev only, no COUNT all); paired with a new composite index on the server, the Records tab loads roughly 5x faster.
- User-Agent reshaped to `defrag-launcher/0.1.9 (windows|macos|linux)` so the website's profile page can label each Connected App by version + platform alongside its IP.

## 0.1.8

- Rich `defrag://` connect modal: bigger map thumbnail, gametype tag, your personal best, current map record, full player list.
- Updater store with a manual "Check now" button in Settings and a 15-minute countdown next to "Automatic updates: on".
- Every update check writes a breadcrumb to `startup.log` (boot / auto / manual) so failed updates are diagnosable from the log.
- Cache normalizes Windows `\\?\` extended paths and corrupted `uploaded.json` is renamed to `.bak.<timestamp>` instead of silently wiped.
- `list_demos` falls back to `queue.json` when the cache is sparse, so the Library tab stops showing "NOT UPLOADED" for demos that were actually uploaded.

## 0.1.7

- Right-click context menu on Library + Demos rows: Open in explorer / Copy path / Copy `/demo` command / Delete.
- Per-row Retry button on error rows so a single failed upload can be re-tried without restarting the whole queue.
- Notifications view redesigned to match the web: record-notification cards with country flag, Q3-colored player name, physics pill, map link, time + diff; system notifications get typed colored icons and sub-tabs.
- `defrag://` URLs now accept hostnames, not just `ip:port`, so `deimos.baseq.fr:27950` works directly.
- Q3 black nicks (`^0`) are outlined with a subtle text-shadow so they stay readable on dark backgrounds.
- Pending `defrag://` modal moved from the Dashboard to App level - it appears as a floating overlay no matter which tab you are on.

## 0.1.6

- WiX installer cleanup: `RemoveFolder` registered for `DefragOrgDir` to satisfy ICE64; AppDataFolder declared under `TARGETDIR` to fix LGHT0094.
- `defrag://` deep-link handler ships: clicking a server join link on the website opens the configured Q3 engine and connects.
- Auto-update via `tauri-plugin-updater` with defrag.racing as the primary endpoint and GitHub Releases as fallback.
- Signed update artifacts via `createUpdaterArtifacts`.
- Optional subfolder watching for the demos directory.
- Persistent upload cache so unchanged demos skip the hash + lookup round-trip on rescan.
- Tray icon + opt-in autostart on login for set-and-forget operation.
- Adaptive CPU throttle (duty cycle) + Speed-up button to cycle throttle tiers.
- Pause halts hashing mid-stream, not just queue draining.
- Persistent upload queue so a launcher restart resumes where it left off.
- Server browser tab with full filter parity to defrag.racing/servers.
- Records + Maps tabs, Profile button, cached `/me` lookup.
- Library tab (demo browser + Render button) + Notifications tab + bell badge.
- Connection history persisted to `history.json`, surfaced as a History tab.
- Onboarding polish and Quick launch top-nav button.

## 0.1.5

- Surface `light.exe` errors on Windows MSI failures (build-time diagnostic step) so WiX bundle failures are debuggable from Actions logs.

## 0.1.4

- Fix WiX fragment - use `DirectoryRef` for `AppDataFolder` to unblock MSI bundling.

## 0.1.3

- Clean-install support on uninstall + reinstall: previously, reinstalling on top of an existing install left stale config behind.

## 0.1.2

- Internal version bump - no user-facing changes shipped under this tag.

## 0.1.1

- Engine detection: scans typical Quake 3 install paths (Steam registry on Windows, common Documents folders elsewhere) so onboarding suggests a sensible engine path.
- Temp folder safety: hashing now uses an OS temp dir instead of writing inside the demos folder.
- File-based token storage with a Reset button in Settings.
- Cross-drive demo scan so users who keep their `defrag/demos/` on a non-system drive work out of the box.
- Surface token errors instead of silently failing.

## 0.1.0

- Initial scaffold: Tauri 2 + Vue 3 + TypeScript shell, basic demo backup pipeline, Settings page.
