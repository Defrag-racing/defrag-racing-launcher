// Open an external URL in the user's default browser.
//
// Routed through our own Rust command (`open_url`) instead of calling
// `@tauri-apps/plugin-opener` directly. On Linux the launcher may run as
// an AppImage, whose AppRun mangles LD_LIBRARY_PATH; the opener plugin
// spawns the browser with that inherited environment and it fails to
// start. The Rust command strips the AppImage additions before spawning
// the system opener, so links work on Linux too. On Windows/macOS the
// command just defers to the opener plugin.

import { invoke } from '@tauri-apps/api/core';

export const openExternal = (url: string): Promise<void> =>
    invoke('open_url', { url });
