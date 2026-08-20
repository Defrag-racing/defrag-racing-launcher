import { computed, ref, type ComputedRef } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

/**
 * Is the UI actually on somebody's screen right now?
 *
 * The launcher does not quit when you close it - it hides into the tray so the
 * demo watcher and the defrag:// handler keep working. The webview stays alive
 * with it, which is what makes reopening instant, and it also means every
 * `setInterval` in the app carries on firing at a window nobody can see. One
 * player read six WebView2 processes off his process list, ticking away with
 * the launcher in the tray and uploads switched off, and asked what they were
 * doing. Redrawing a dashboard for nobody, mostly.
 *
 * Two signals, and a poll should stop on either:
 *
 * - `launcher-visible`, emitted by us on every hide and show (see
 *   `hide_to_tray` / `show_main_window` in lib.rs). This is the tray case, and
 *   it is the exact one, because whether a hidden native window reports itself
 *   hidden to the page is up to the webview and not a thing to build on.
 * - `document.hidden`, which covers what the webview does know about - the
 *   window being fully covered or the machine going to sleep.
 *
 * The starting value is read from the window rather than assumed, because the
 * launcher can start hidden: an autostart run comes up in the tray, and a UI
 * that assumes it is visible until told otherwise polls from boot until the
 * user first opens it.
 */
const shown = ref(true);
const painted = ref(!document.hidden);

let started = false;

function start(): void {
    if (started) return;
    started = true;

    void getCurrentWindow()
        .isVisible()
        .then((v) => { shown.value = v; })
        .catch(() => { /* not in a window: leave it visible */ });

    void listen<boolean>('launcher-visible', (e) => { shown.value = e.payload; });

    document.addEventListener('visibilitychange', () => { painted.value = !document.hidden; });
}

/**
 * Never unlistened. There is one window and one document for the life of the
 * process, so this is set up once and read by every view; tearing it down with
 * a component would just mean the next component to mount rebuilds it.
 */
export function useOnScreen(): ComputedRef<boolean> {
    start();

    return computed(() => shown.value && painted.value);
}
