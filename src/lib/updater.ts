// Thin async helper around @tauri-apps/plugin-updater so the Vue side
// doesn't repeat the check/download/install dance.
//
// Flow we implement:
//   1. check() — hits the endpoints in tauri.conf.json (defrag.racing
//      first, GH Releases second). Returns the Update object if a newer
//      version exists, null otherwise.
//   2. download() — pulls the platform-appropriate bundle and verifies
//      the signature against the embedded pubkey before storing it.
//   3. install() + relaunch() — applies the bundle and restarts the
//      launcher so users land on the new version immediately.
//
// We do NOT use the plugin's built-in dialog (dialog: false in config)
// because we want the toast to live inside our own UI alongside the
// upload queue and deep-link toasts.

import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

export type UpdateState =
    | { kind: 'idle' }
    | { kind: 'checking' }
    | { kind: 'available'; version: string }
    | { kind: 'downloading'; percent: number }
    | { kind: 'installing' }
    | { kind: 'error'; message: string };

export async function checkForUpdate(): Promise<Update | null> {
    // Tauri returns null when we're already on the latest version, an
    // Update object otherwise. Errors here usually mean "no network" or
    // "endpoint 404" — both worth surfacing as a non-blocking warning
    // rather than swallowing.
    return await check();
}

/**
 * Run check → download → install → relaunch. Caller passes a callback
 * that receives state transitions so the UI can render progress and
 * eventual success/failure.
 */
export async function runUpdate(
    update: Update,
    onState: (s: UpdateState) => void,
): Promise<void> {
    try {
        let downloaded = 0;
        let contentLength = 0;
        onState({ kind: 'downloading', percent: 0 });

        await update.downloadAndInstall((event) => {
            switch (event.event) {
                case 'Started':
                    contentLength = event.data.contentLength ?? 0;
                    break;
                case 'Progress':
                    downloaded += event.data.chunkLength;
                    if (contentLength > 0) {
                        onState({
                            kind: 'downloading',
                            percent: Math.min(100, Math.round((downloaded / contentLength) * 100)),
                        });
                    }
                    break;
                case 'Finished':
                    onState({ kind: 'installing' });
                    break;
            }
        });

        // downloadAndInstall returns once the new binary is in place;
        // relaunch swaps the running process for it. The user will see
        // the launcher disappear and reappear on the new version.
        await relaunch();
    } catch (e: any) {
        onState({ kind: 'error', message: e?.toString?.() ?? String(e) });
    }
}
