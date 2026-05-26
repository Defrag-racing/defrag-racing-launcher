<script setup lang="ts">
    // Defrag:// connection history. Reads history.json (rolling, last
    // 200 entries) and renders a "where did I join, when" list. Each
    // row shows timestamp + server (with Q3 colors when we logged a
    // name) + map + ip:port; clicking the row re-connects to the same
    // address via the existing protocol handler.

    import { computed, onActivated, onMounted, ref } from 'vue';
    import { tauri, type ConnectionEntry } from '../lib/tauri';
    import { q3ToHtml } from '../lib/q3color';
    import { openUrl } from '@tauri-apps/plugin-opener';

    const entries = ref<ConnectionEntry[]>([]);
    const loading = ref(true);
    const error = ref<string | null>(null);
    const reconnecting = ref<string | null>(null);

    const refresh = async () => {
        loading.value = true;
        error.value = null;
        try {
            entries.value = await tauri.getConnectionHistory();
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to load history';
        } finally {
            loading.value = false;
        }
    };

    onMounted(refresh);
    // keep-alive re-entry: re-read history.json so a defrag:// click
    // that happened while the user was on a different tab shows up
    // immediately when they switch back here.
    onActivated(() => { void refresh(); });

    const clearAll = async () => {
        if (!confirm('Clear all connection history?')) return;
        try {
            await tauri.clearConnectionHistory();
            entries.value = [];
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to clear history';
        }
    };

    const reconnect = async (e: ConnectionEntry) => {
        const key = `${e.ip}:${e.port}#${e.timestamp_ms}`;
        reconnecting.value = key;
        try {
            await tauri.handleProtocolUrl(`defrag://${e.ip}:${e.port}`);
            // No history.log call here - handleProtocolUrl reaches the
            // engine directly and doesn't go through the
            // confirm_pending_deep_link command that logs. If we
            // wanted these "reconnect" events in history we'd add a
            // dedicated command; skipping for now to avoid log spam
            // from accidental double-clicks.
        } catch (err: any) {
            error.value = err?.toString?.() ?? 'Connect failed';
        } finally {
            reconnecting.value = null;
        }
    };

    const openMap = (mapname: string | null) => {
        if (!mapname) return;
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`)
            .catch(() => { /* best effort */ });
    };

    // Human-friendly relative time. Switches to absolute date once the
    // entry is older than a few days, since "37 hours ago" stops being
    // useful before that.
    const formatTime = (ms: number): string => {
        const diff = Date.now() - ms;
        const s = Math.round(diff / 1000);
        if (s < 60) return `${s}s ago`;
        const m = Math.round(s / 60);
        if (m < 60) return `${m} min ago`;
        const h = Math.round(m / 60);
        if (h < 48) return `${h}h ago`;
        return new Date(ms).toLocaleString();
    };

    // Ticking ref so relative times update without manual refresh.
    const _now = ref(Date.now());
    let nowTimer: number | undefined;
    onMounted(() => {
        nowTimer = window.setInterval(() => { _now.value = Date.now(); }, 30_000);
    });
    import { onUnmounted } from 'vue';
    onUnmounted(() => {
        if (nowTimer !== undefined) window.clearInterval(nowTimer);
    });

    const labelFor = computed(() => (e: ConnectionEntry) => {
        void _now.value; // re-evaluate every tick
        return formatTime(e.timestamp_ms);
    });

    const stripColors = (s: string): string =>
        s.replace(/\^\d|\^x[\da-fA-F]{2}|\^[\da-fA-F]{6}/g, '');
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">History</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    Servers you joined via <code class="bg-black/40 px-1 rounded">defrag://</code> links.
                    Newest first. Click a row to reconnect.
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs flex-shrink-0">
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading"
                    @click="refresh"
                >{{ loading ? 'Loading…' : 'Refresh' }}</button>
                <button
                    v-if="entries.length"
                    class="px-2 py-1 rounded bg-white/5 hover:bg-red-500/20 text-neutral-400 hover:text-red-300"
                    @click="clearAll"
                >Clear</button>
            </div>
        </header>

        <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ error }}
        </p>

        <div class="flex-1 overflow-auto">
            <div v-if="loading && !entries.length" class="p-8 text-center text-sm text-neutral-500">
                Loading…
            </div>
            <div v-else-if="!entries.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-5xl">🕒</div>
                    <div class="text-neutral-300 font-semibold">No connections yet</div>
                    <p class="text-sm text-neutral-500">
                        Click a <code class="bg-black/40 px-1 rounded">defrag://</code> link on a
                        defrag.racing server card to join one - it'll show up here.
                    </p>
                </div>
            </div>

            <ul v-else class="divide-y divide-white/[0.04]">
                <li
                    v-for="(e, idx) in entries"
                    :key="`${e.ip}:${e.port}#${e.timestamp_ms}#${idx}`"
                    class="px-5 py-3 flex items-start gap-3"
                >
                    <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2 min-w-0">
                            <div
                                v-if="e.server_name"
                                class="text-sm text-neutral-100 truncate font-semibold"
                                :title="stripColors(e.server_name)"
                                v-html="q3ToHtml(e.server_name)"
                            ></div>
                            <div v-else class="text-sm text-neutral-300 font-mono">
                                {{ e.ip }}:{{ e.port }}
                            </div>
                            <span
                                v-if="e.physics"
                                class="uppercase text-[10px] px-1 py-0.5 rounded bg-white/5 text-neutral-300 flex-shrink-0"
                            >{{ e.physics }}</span>
                            <span
                                class="uppercase text-[10px] px-1 py-0.5 rounded flex-shrink-0"
                                :class="e.source === 'auto' ? 'bg-amber-500/10 text-amber-300' : 'bg-brand-500/10 text-brand-300'"
                                :title="e.source === 'auto' ? 'Auto-connect (Settings)' : 'You pressed Connect'"
                            >{{ e.source }}</span>
                        </div>
                        <div class="text-xs text-neutral-500 truncate flex items-center gap-2 mt-0.5">
                            <button
                                v-if="e.map"
                                class="text-brand-400 hover:underline"
                                @click="openMap(e.map)"
                            >{{ e.map }}</button>
                            <span v-if="e.map" class="text-neutral-600">·</span>
                            <span v-if="e.server_name" class="font-mono">{{ e.ip }}:{{ e.port }}</span>
                            <span v-if="e.server_name" class="text-neutral-600">·</span>
                            <span>{{ labelFor(e) }}</span>
                        </div>
                    </div>
                    <button
                        class="px-3 py-1 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-xs font-semibold disabled:opacity-50 flex-shrink-0"
                        :disabled="reconnecting === `${e.ip}:${e.port}#${e.timestamp_ms}`"
                        @click="reconnect(e)"
                    >Reconnect</button>
                </li>
            </ul>
        </div>
    </div>
</template>
