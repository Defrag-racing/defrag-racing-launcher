import { defineStore } from 'pinia';
import { ref } from 'vue';
import { tauri } from '../lib/tauri';
import { useConfigStore } from './config';

/** Shared unread counts for the bell badge. Refreshed every 90s by
 *  App.vue and mutated optimistically by the Notifications view when
 *  the user toggles read state, so the badge stays in sync without a
 *  round-trip per click. */
export const useNotificationsStore = defineStore('notifications', () => {
    const records = ref(0);
    const system = ref(0);
    const total = ref(0);

    const set = (r: number, s: number) => {
        records.value = r;
        system.value = s;
        total.value = r + s;
    };

    // Bell-badge refresh - hits the lightweight unread-count endpoint
    // (~30B JSON). Falls back to the full /notifications endpoint when
    // the user's web is still on the old build that doesn't have the
    // new route. Once everyone upgrades the fallback can go.
    const refresh = async () => {
        const config = useConfigStore();
        if (!config.hasToken) { set(0, 0); return; }
        try {
            const counts = await tauri.getNotificationsUnreadCount();
            set(counts.records, counts.system);
            return;
        } catch {
            // fall through to full fetch
        }
        try {
            const feed = await tauri.getNotifications();
            set(feed.unread.records, feed.unread.system);
        } catch {
            // ignore - retry next tick
        }
    };

    return { records, system, total, set, refresh };
});
