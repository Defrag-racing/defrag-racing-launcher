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

    const refresh = async () => {
        const config = useConfigStore();
        if (!config.hasToken) { set(0, 0); return; }
        try {
            const feed = await tauri.getNotifications();
            set(feed.unread.records, feed.unread.system);
        } catch {
            // ignore - retry next tick
        }
    };

    return { records, system, total, set, refresh };
});
