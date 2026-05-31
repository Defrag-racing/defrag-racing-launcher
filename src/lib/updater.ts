// Thin async helper around @tauri-apps/plugin-updater so the Vue side
// doesn't repeat the check/download/install dance.
//
// Flow we implement:
//   1. check() - hits the endpoints in tauri.conf.json (defrag.racing
//      first, GH Releases second). Returns the Update object if a newer
//      version exists, null otherwise.
//   2. download() - pulls the platform-appropriate bundle and verifies
//      the signature against the embedded pubkey before storing it.
//   3. install() + relaunch() - applies the bundle and restarts the
//      launcher so users land on the new version immediately.
//
// We do NOT use the plugin's built-in dialog (dialog: false in config)
// because we want the toast to live inside our own UI alongside the
// upload queue and deep-link toasts.

import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { tauri } from './tauri';

export type UpdateState =
    | { kind: 'idle' }
    | { kind: 'checking' }
    | { kind: 'available'; version: string }
    | { kind: 'downloading'; percent: number }
    | { kind: 'installing' }
    | { kind: 'error'; message: string };

const logUpdate = (msg: string) => {
    // Fire-and-forget; we don't want the logging round-trip to slow
    // down the user-visible update path.
    void tauri.logToFile(`updater: ${msg}`);
};

/** How the check was triggered - logged into startup.log so the user
 *  can tell apart "interval tick fired" vs "I clicked the button" when
 *  diagnosing why a release didn't roll out. */
export type CheckSource = 'auto' | 'manual' | 'boot';

export async function checkForUpdate(source: CheckSource = 'auto'): Promise<Update | null> {
    // Tauri returns null when we're already on the latest version, an
    // Update object otherwise. Errors here usually mean "no network" or
    // "endpoint 404" - both worth surfacing as a non-blocking warning
    // rather than swallowing.
    logUpdate(`check() called (${source})`);
    try {
        const result = await check();
        if (result) {
            logUpdate(`check() ok (${source}) - update available: v${result.version}`);
        } else {
            logUpdate(`check() ok (${source}) - no update (already on latest)`);
        }
        return result;
    } catch (e: any) {
        logUpdate(`check() FAILED (${source}): ${e?.toString?.() ?? String(e)}`);
        throw e;
    }
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
    logUpdate(`runUpdate starting for v${update.version}`);
    try {
        let downloaded = 0;
        let contentLength = 0;
        onState({ kind: 'downloading', percent: 0 });

        await update.downloadAndInstall((event) => {
            switch (event.event) {
                case 'Started':
                    contentLength = event.data.contentLength ?? 0;
                    logUpdate(`download started, ${contentLength} bytes`);
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
                    logUpdate('download finished, installing');
                    onState({ kind: 'installing' });
                    break;
            }
        });

        logUpdate('install ok, relaunching');
        // downloadAndInstall returns once the new binary is in place;
        // relaunch swaps the running process for it. The user will see
        // the launcher disappear and reappear on the new version.
        await relaunch();
    } catch (e: any) {
        logUpdate(`runUpdate FAILED: ${e?.toString?.() ?? String(e)}`);
        onState({ kind: 'error', message: e?.toString?.() ?? String(e) });
    }
}
