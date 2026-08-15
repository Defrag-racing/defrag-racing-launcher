<script setup lang="ts">
    // Defrag:// connection history. Reads history.json (rolling, last
    // 200 entries) and renders a "where did I join, when" list. Each
    // row shows timestamp + server (with Q3 colors when we logged a
    // name) + map + ip:port; clicking the row re-connects to the same
    // address via the existing protocol handler.

    import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref } from 'vue';
    import { tauri, type ConnectionEntry } from '../lib/tauri';
    import { q3ToHtml } from '../lib/q3color';
    import { openExternal } from '../lib/open';
    import { t } from '../lib/i18n';

    const entries = ref<ConnectionEntry[]>([]);
    const loading = ref(true);
    const error = ref<string | null>(null);
    const reconnecting = ref<string | null>(null);

    // `silent` skips the loading spinner so the auto-refresh tick doesn't
    // flash "Loading…" over a list that's already on screen.
    const refresh = async (silent = false) => {
        if (!silent) loading.value = true;
        error.value = null;
        try {
            entries.value = await tauri.getConnectionHistory();
        } catch (e: any) {
            error.value = e?.toString?.() ?? t('Failed to load history');
        } finally {
            loading.value = false;
        }
    };

    onMounted(() => { void refresh(); });
    // keep-alive re-entry: re-read history.json so a defrag:// click
    // that happened while the user was on a different tab shows up
    // immediately when they switch back here.
    const viewActive = ref(true);
    onActivated(() => { viewActive.value = true; void refresh(); });
    onDeactivated(() => { viewActive.value = false; });

    // Auto-refresh every 30s, but only while this view is the one on
    // screen AND the launcher window is actually focused/visible - no
    // point hammering history.json when the user is in another app or on
    // a different tab. Uses a silent refresh so the list doesn't flicker.
    let autoTimer: number | undefined;
    const windowIsActive = (): boolean =>
        document.visibilityState === 'visible' && document.hasFocus();
    onMounted(() => {
        autoTimer = window.setInterval(() => {
            if (viewActive.value && windowIsActive()) void refresh(true);
        }, 30_000);
    });
    onUnmounted(() => {
        if (autoTimer !== undefined) window.clearInterval(autoTimer);
    });

    // Two-step clear: first click arms the inline confirm, second click
    // (the red "Clear all" button) actually wipes. Replaces window.confirm,
    // which is unreliable inside the WebView (returns immediately on some
    // platforms, so history got nuked with no prompt).
    const confirmingClear = ref(false);
    let confirmTimer: number | undefined;
    const askClear = () => {
        confirmingClear.value = true;
        // auto-disarm after a few seconds so a stray armed state doesn't
        // linger and catch the next click.
        if (confirmTimer !== undefined) window.clearTimeout(confirmTimer);
        confirmTimer = window.setTimeout(() => { confirmingClear.value = false; }, 5_000);
    };
    const cancelClear = () => {
        confirmingClear.value = false;
        if (confirmTimer !== undefined) window.clearTimeout(confirmTimer);
    };
    const clearAll = async () => {
        confirmingClear.value = false;
        if (confirmTimer !== undefined) window.clearTimeout(confirmTimer);
        try {
            await tauri.clearConnectionHistory();
            entries.value = [];
        } catch (e: any) {
            error.value = e?.toString?.() ?? t('Failed to clear history');
        }
    };

    const reconnect = async (e: ConnectionEntry) => {
        const key = `${e.ip}:${e.port}#${e.timestamp_ms}`;
        reconnecting.value = key;
        try {
            await tauri.handleProtocolUrl(
                `defrag://${e.ip}:${e.port}`,
                {
                    map: e.map ?? null,
                    server_name: e.server_name ?? null,
                    physics: e.physics ?? null,
                },
                'reconnect',
            );
            await refresh();
        } catch (err: any) {
            error.value = err?.toString?.() ?? t('Connect failed');
        } finally {
            reconnecting.value = null;
        }
    };

    const openMap = (mapname: string | null) => {
        if (!mapname) return;
        openExternal(`https://defrag.racing/maps/${encodeURIComponent(mapname)}`)
            .catch(() => { /* best effort */ });
    };

    // Human-friendly relative time. Switches to absolute date once the
    // entry is older than a few days, since "37 hours ago" stops being
    // useful before that.
    const formatTime = (ms: number): string => {
        const diff = Date.now() - ms;
        const s = Math.round(diff / 1000);
        if (s < 60) return t(':count seconds ago', { count: s });
        const m = Math.round(s / 60);
        if (m < 60) return t(':count minutes ago', { count: m });
        const h = Math.round(m / 60);
        if (h < 48) return t(':count hours ago', { count: h });
        return new Date(ms).toLocaleString();
    };

    // Ticking ref so relative times update without manual refresh.
    const _now = ref(Date.now());
    let nowTimer: number | undefined;
    onMounted(() => {
        nowTimer = window.setInterval(() => { _now.value = Date.now(); }, 30_000);
    });
    onUnmounted(() => {
        if (nowTimer !== undefined) window.clearInterval(nowTimer);
    });

    const labelFor = computed(() => (e: ConnectionEntry) => {
        void _now.value; // re-evaluate every tick
        return formatTime(e.timestamp_ms);
    });

    // Relative-time label for an arbitrary timestamp (the per-map plays),
    // ticking off the same _now ref as the row labels.
    const relTimeMs = computed(() => (ms: number) => {
        void _now.value;
        return formatTime(ms);
    });

    const stripColors = (s: string): string =>
        s.replace(/\^\d|\^x[\da-fA-F]{2}|\^[\da-fA-F]{6}/g, '');

    // Stable key per entry (the backend session id; falls back to
    // ip:port#ts for legacy entries written before ids existed).
    const rowKey = (e: ConnectionEntry): string =>
        e.id || `${e.ip}:${e.port}#${e.timestamp_ms}`;

    // Which rows have their map timeline expanded.
    const expanded = ref<Set<string>>(new Set());
    const toggleExpand = (e: ConnectionEntry) => {
        const k = rowKey(e);
        const next = new Set(expanded.value);
        if (next.has(k)) next.delete(k); else next.add(k);
        expanded.value = next;
    };
    const isExpanded = (e: ConnectionEntry): boolean => expanded.value.has(rowKey(e));
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">{{ $t('History') }}</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    {{ $t('Servers you joined via defrag:// links. Newest first. Click a row to reconnect.') }}
                </div>
            </div>
            <div class="flex items-center gap-2 text-xs flex-shrink-0">
                <button
                    class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-50"
                    :disabled="loading"
                    @click="refresh()"
                >{{ loading ? $t('Loading…') : $t('Refresh') }}</button>
                <template v-if="entries.length">
                    <button
                        v-if="!confirmingClear"
                        class="px-2 py-1 rounded bg-white/5 hover:bg-red-500/20 text-neutral-400 hover:text-red-300"
                        @click="askClear"
                    >{{ $t('Clear') }}</button>
                    <template v-else>
                        <span class="text-neutral-400">{{ $t('Clear all history?') }}</span>
                        <button
                            class="px-2 py-1 rounded bg-red-500/20 hover:bg-red-500/30 text-red-300 font-semibold"
                            @click="clearAll"
                        >{{ $t('Clear all') }}</button>
                        <button
                            class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300"
                            @click="cancelClear"
                        >{{ $t('Cancel') }}</button>
                    </template>
                </template>
            </div>
        </header>

        <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ error }}
        </p>

        <div class="flex-1 overflow-auto">
            <div v-if="loading && !entries.length" class="p-8 text-center text-sm text-neutral-500">
                {{ $t('Loading…') }}
            </div>
            <div v-else-if="!entries.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-5xl">🕒</div>
                    <div class="text-neutral-300 font-semibold">{{ $t('No connections yet') }}</div>
                    <p class="text-sm text-neutral-500">
                        {{ $t("Click a defrag:// link on a defrag.racing server card to join one - it will show up here.") }}
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
                                :title="e.source === 'auto' ? $t('Auto-connect (Settings)') : $t('You pressed Connect')"
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
                            <button
                                v-if="e.maps_played && e.maps_played.length"
                                class="ml-1 text-neutral-400 hover:text-neutral-200 flex items-center gap-0.5"
                                :title="isExpanded(e) ? $t('Hide maps played') : $t('Show maps played')"
                                @click="toggleExpand(e)"
                            >
                                <span class="inline-block transition-transform" :class="isExpanded(e) ? 'rotate-90' : ''">▸</span>
                                {{ e.maps_played.length }} {{ e.maps_played.length === 1 ? $t('map') : $t('maps') }}
                            </button>
                        </div>

                        <!-- Per-session map timeline: maps the server
                             rotated through while the game was running.
                             Chronological (join map first). -->
                        <ul
                            v-if="isExpanded(e) && e.maps_played && e.maps_played.length"
                            class="mt-2 ml-1 border-l border-white/[0.06] pl-3 space-y-1"
                        >
                            <li
                                v-for="(mp, i) in e.maps_played"
                                :key="`${mp.map}#${mp.timestamp_ms}#${i}`"
                                class="flex items-center gap-2 text-xs"
                            >
                                <button class="text-brand-400 hover:underline truncate" @click="openMap(mp.map)">{{ mp.map }}</button>
                                <span
                                    v-if="mp.physics"
                                    class="uppercase text-[10px] px-1 py-0.5 rounded bg-white/5 text-neutral-300 flex-shrink-0"
                                >{{ mp.physics }}</span>
                                <span class="text-neutral-600 ml-auto whitespace-nowrap">{{ relTimeMs(mp.timestamp_ms) }}</span>
                            </li>
                        </ul>
                    </div>
                    <button
                        class="px-3 py-1 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 text-xs font-semibold disabled:opacity-50 flex-shrink-0"
                        :disabled="reconnecting === `${e.ip}:${e.port}#${e.timestamp_ms}`"
                        @click="reconnect(e)"
                    >{{ $t('Reconnect') }}</button>
                </li>
            </ul>
        </div>
    </div>
</template>
