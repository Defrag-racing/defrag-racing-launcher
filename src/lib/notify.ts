// Desktop notifications: the launcher talking to somebody who is not looking
// at it.
//
// That is the normal case. The launcher spends its life minimised behind a
// fullscreen Quake, and everything it has to say is time-sensitive - a round
// that just opened, a demo sitting held waiting for an answer, somebody taking
// a record off you. A message that waits in the window until the next alt-tab
// is a message that arrived too late.
//
// Permission is asked for lazily, on the first notification we actually want to
// send, rather than at startup. Asking during onboarding would be asking about
// something the user has not been shown yet, and a permission prompt with no
// notification behind it reads as a nag.

import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
} from '@tauri-apps/plugin-notification';
import { LazyStore } from '@tauri-apps/plugin-store';

/** Which switch in Settings governs a message. */
export type NotifyCategory = 'comps' | 'records' | 'system';

let granted: boolean | null = null;

/** Ask once per session, remember the answer for the rest of it.
 *
 *  A refusal is not retried: the OS shows its own prompt, and asking again on
 *  the next event would be pestering somebody who already said no. Turning the
 *  switch off in Settings is the way to stop being asked at all. */
const ensurePermission = async (): Promise<boolean> => {
    if (granted !== null) return granted;
    try {
        granted = await isPermissionGranted();
        if (!granted) granted = (await requestPermission()) === 'granted';
    } catch {
        granted = false;
    }
    return granted;
};

/** Fire one notification. Never throws: a failed notification must not take
 *  down the poll that produced it. */
export const notify = async (title: string, body: string): Promise<void> => {
    try {
        if (!(await ensurePermission())) return;
        sendNotification({ title, body });
    } catch {
        // No notification service, permission revoked mid-session, headless
        // session - all of them mean "not shown", none of them mean "broken".
    }
};

// ---- what has already been said ------------------------------------
//
// Persisted, not kept in memory: without this every restart would replay the
// last round's news, and a launcher that starts with your machine would do it
// every morning.
//
// A plugin-store file rather than localStorage, which does not survive a
// restart in this webview.

interface NotifySeen {
    /** Highest record-notification id already announced. */
    record: number;
    /** Highest system-notification id already announced. */
    system: number;
    /** The comps round we last announced as open. */
    round: number;
    /** The round the user had an entry in, so its ending can be announced. */
    enteredRound: number;
    /** Entry ids whose verdict has already been announced. */
    settled: number[];
}

const BLANK: NotifySeen = { record: 0, system: 0, round: 0, enteredRound: 0, settled: [] };

const store = new LazyStore('notify-seen.json');
const FIELD = 'seen';

let cache: NotifySeen | null = null;

export const loadSeen = async (): Promise<NotifySeen> => {
    if (cache) return cache;
    try {
        const v = await store.get<Partial<NotifySeen>>(FIELD);
        cache = { ...BLANK, ...(v ?? {}) };
    } catch {
        cache = { ...BLANK };
    }
    return cache;
};

export const saveSeen = async (patch: Partial<NotifySeen>): Promise<void> => {
    const current = await loadSeen();
    cache = { ...current, ...patch };
    // Keep the settled list from growing without bound. Entries are announced
    // once and never revisited, so anything older than the last few rounds'
    // worth is dead weight.
    if (cache.settled.length > 50) cache.settled = cache.settled.slice(-50);
    try {
        await store.set(FIELD, cache);
        await store.save();
    } catch {
        // Worst case the same thing is announced twice after a restart.
    }
};

export type { NotifySeen };
