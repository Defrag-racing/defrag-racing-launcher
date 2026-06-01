# Changelog

All notable changes to the Defrag Racing Launcher.

The format roughly follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
