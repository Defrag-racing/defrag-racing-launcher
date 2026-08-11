# Defrag Racing Launcher

Desktop companion app for [defrag.racing](https://defrag.racing). Runs in the tray, automatically backs up new Quake 3 Defrag demos to your account, and opens `defrag://` links from the website directly in your engine of choice (oDFe or iDFe).

Built with [Tauri 2](https://tauri.app), Vue 3, and Rust.

## What it does

- **Auto demo backup** — watches your `defrag/demos/` folder and uploads new demos to defrag.racing. Deduplicated by MD5, so re-scanning is free. Opt-in; requires a personal access token from the website.
- **Engine auto-detect** — scans the usual install paths on Windows, macOS and Linux (including Steam, Flatpak) for oDFe and iDFe. Never picks for you — always shows a list and lets you choose.
- **`defrag://` protocol handler** — clicking a server link on defrag.racing (`defrag://1.2.3.4:27960`) opens the configured engine with `+connect 1.2.3.4:27960` and brings the connect prompt up immediately.

The auth token is stored in the OS keyring (Windows Credential Manager / macOS Keychain / libsecret). No secrets on disk.

## Install

Grab the installer for your platform from [Releases](https://github.com/Defrag-racing/defrag-racing-launcher/releases):

- **Windows**: `.exe` (installs per user, so it never asks for admin rights)
- **macOS**: `.dmg` (pick Apple Silicon for M1/M2/M3, Intel for older Macs)
- **Linux**: `.AppImage` (portable) or `.deb`

### Scoop

This repository doubles as a [Scoop](https://scoop.sh) bucket:

```powershell
scoop bucket add defrag https://github.com/Defrag-racing/defrag-racing-launcher
scoop install defrag-racing-launcher
```

It runs the same per-user installer as a manual download, so the `defrag://`
protocol handler and the Start menu entry are registered exactly as usual. The
manifest is repointed at each new release by the tag workflow, so
`scoop update` always finds the latest build.

One thing to expect: the launcher updates itself on launch, so the version
Scoop has on record can trail the one you are actually running. Nothing breaks,
and `scoop update` remains harmless.

After installing, open your defrag.racing profile → Settings → Security → Launcher tokens, generate a token, and paste it into the launcher on first run.

Updates are automatic — future versions install themselves on launch (you can opt out in settings).

## Development

Prerequisites:
- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 20+
- Platform deps for Tauri: see [tauri.app/start/prerequisites/](https://tauri.app/start/prerequisites/)

```bash
npm install

# Run against production defrag.racing
npm run tauri dev

# Run against a local Laravel instance
DEFRAG_API_URL=http://localhost npm run tauri dev

# Build a release bundle (installer for the current OS)
npm run tauri build
```

## Releasing

Push a tag named `launcher-vX.Y.Z` to trigger the matrix build workflow:

```bash
git tag launcher-v0.1.0
git push --tags
```

GitHub Actions builds Windows / macOS (Intel + Apple Silicon) / Linux installers in parallel and attaches them to a draft GitHub Release. Review the release and mark it published when ready.

## Rolling back a bad release

Tauri updater goes forward-only by default. To roll users back:

1. **Re-release trick**: tag `launcher-v0.1.3` that checks out `v0.1.1` code, bumps the package version to `0.1.3` and releases. Users on the broken `0.1.2` auto-update to the identical-code-but-higher-version `0.1.3`.
2. **Delete the release on GitHub**: only affects new installs; does not roll back existing clients.

A proper kill-switch manifest endpoint on defrag.racing can be added later if we need true downgrade enforcement.

## License

MIT. See [LICENSE](LICENSE).
