# Changelog

All notable changes to the Defrag Racing Launcher.

The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
