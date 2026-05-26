<script setup lang="ts">
    // Recent records browser. Two side-by-side paginated tables (VQ3
    // + CPM) so the user can scan the latest activity at a glance.
    // Deliberately a minimal projection of the web /rankings page -
    // no MapFilters surface, no rating context, no offline records
    // merge. The launcher's job is the quick browse; users who want
    // the full rating UI click a map or nickname through to the web.

    import { onActivated, onDeactivated, onMounted, onUnmounted, ref } from 'vue';
    import { tauri, type RecordRow, type Paginated } from '../lib/tauri';
    import { q3ToHtml } from '../lib/q3color';
    import { useConfigStore } from '../stores/config';
    import { openUrl } from '@tauri-apps/plugin-opener';

    const config = useConfigStore();

    const vq3 = ref<Paginated<RecordRow> | null>(null);
    const cpm = ref<Paginated<RecordRow> | null>(null);
    const vq3Page = ref(1);
    const cpmPage = ref(1);
    const vq3Loading = ref(false);
    const cpmLoading = ref(false);
    const error = ref<string | null>(null);

    const load = async (physics: 'vq3' | 'cpm', page: number) => {
        const loadingRef = physics === 'vq3' ? vq3Loading : cpmLoading;
        const dataRef = physics === 'vq3' ? vq3 : cpm;
        loadingRef.value = true;
        error.value = null;
        try {
            dataRef.value = await tauri.getRecords(physics, page);
        } catch (e: any) {
            error.value = e?.toString?.() ?? 'Failed to load records';
        } finally {
            loadingRef.value = false;
        }
    };

    const refreshBoth = () => {
        if (!config.hasToken) return;
        void load('vq3', vq3Page.value);
        void load('cpm', cpmPage.value);
    };

    const POLL_MS = 60_000;
    let pollTimer: number | undefined;
    const startPolling = () => {
        stopPolling();
        pollTimer = window.setInterval(() => {
            if (!document.hidden) refreshBoth();
        }, POLL_MS);
    };
    const stopPolling = () => {
        if (pollTimer !== undefined) { window.clearInterval(pollTimer); pollTimer = undefined; }
    };

    const onVisibility = () => {
        if (document.hidden) stopPolling();
        else { refreshBoth(); startPolling(); }
    };

    onMounted(() => {
        refreshBoth();
        startPolling();
        document.addEventListener('visibilitychange', onVisibility);
    });
    onActivated(() => {
        refreshBoth();
        startPolling();
    });
    onDeactivated(() => stopPolling());
    onUnmounted(() => {
        stopPolling();
        document.removeEventListener('visibilitychange', onVisibility);
    });

    const setVq3Page = (p: number) => {
        if (p < 1 || (vq3.value && p > vq3.value.last_page)) return;
        vq3Page.value = p;
        void load('vq3', p);
    };
    const setCpmPage = (p: number) => {
        if (p < 1 || (cpm.value && p > cpm.value.last_page)) return;
        cpmPage.value = p;
        void load('cpm', p);
    };

    const stripColors = (s: string): string =>
        s.replace(/\^\d|\^x[\da-fA-F]{2}|\^[\da-fA-F]{6}/g, '');

    const flagUrl = (country: string | null | undefined): string | null => {
        if (!country) return null;
        if (country === '_404' || country === 'XX') return null;
        return `https://defrag.racing/images/flags/${country.toLowerCase()}.png`;
    };

    /** Defrag stores times as milliseconds; format MM:SS.mmm / SS.mmm. */
    const formatTime = (ms: number | null | undefined): string => {
        if (ms == null || ms <= 0) return '-';
        const totalSec = Math.floor(ms / 1000);
        const m = Math.floor(totalSec / 60);
        const s = totalSec % 60;
        const mmm = ms % 1000;
        if (m > 0) return `${m}:${s.toString().padStart(2, '0')}.${mmm.toString().padStart(3, '0')}`;
        return `${s}.${mmm.toString().padStart(3, '0')}`;
    };

    const formatDate = (s: string | null): string => {
        if (!s) return '';
        const d = new Date(s.replace(' ', 'T') + 'Z');
        return isNaN(d.getTime()) ? s : d.toLocaleDateString();
    };

    const openMap = (name: string) => {
        openUrl(`https://defrag.racing/maps/${encodeURIComponent(name)}`)
            .catch(() => { /* best effort */ });
    };

    const openProfile = (mddId: number | null) => {
        if (!mddId) return;
        openUrl(`https://defrag.racing/profile/${mddId}`)
            .catch(() => { /* best effort */ });
    };
</script>

<template>
    <div class="flex-1 flex flex-col min-h-0">
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between gap-3">
            <div class="min-w-0">
                <div class="font-semibold">Records</div>
                <div class="text-xs text-neutral-500 mt-0.5 truncate">
                    Recent records from defrag.racing, newest first. Click a name or
                    map to open the full page on the web.
                </div>
            </div>
        </header>

        <div v-if="!config.hasToken" class="flex-1 flex items-center justify-center p-8">
            <div class="text-center max-w-sm space-y-2">
                <div class="text-5xl">🔑</div>
                <div class="text-neutral-300 font-semibold">Token required</div>
                <p class="text-sm text-neutral-500">
                    Records browser needs a token from your defrag.racing account.
                    Open Settings to paste one.
                </p>
            </div>
        </div>

        <template v-else>
            <p v-if="error" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
                {{ error }}
            </p>

            <div class="flex-1 overflow-auto p-3 grid grid-cols-1 xl:grid-cols-2 gap-3">
                <!-- VQ3 table -->
                <section class="bg-neutral-900/40 border border-white/10 rounded-lg flex flex-col min-h-0">
                    <header class="px-3 py-2 border-b border-white/10 flex items-center justify-between">
                        <div class="text-sm font-semibold text-neutral-200">VQ3</div>
                        <div class="text-xs text-neutral-500" v-if="vq3">
                            page {{ vq3.current_page }} / {{ vq3.last_page }} · {{ vq3.total }} total
                        </div>
                    </header>
                    <div class="flex-1 overflow-auto">
                        <div v-if="vq3Loading && !vq3" class="p-6 text-center text-xs text-neutral-500">
                            Loading…
                        </div>
                        <table v-else-if="vq3" class="w-full text-xs">
                            <thead class="text-[10px] uppercase text-neutral-500 sticky top-0 bg-neutral-900/95 backdrop-blur">
                                <tr>
                                    <th class="text-left px-2 py-1.5 font-normal">Date</th>
                                    <th class="text-left px-2 py-1.5 font-normal">Player</th>
                                    <th class="text-right px-2 py-1.5 font-normal">Time</th>
                                    <th class="text-right px-2 py-1.5 font-normal">Rank</th>
                                    <th class="text-left px-2 py-1.5 font-normal">Map</th>
                                </tr>
                            </thead>
                            <tbody>
                                <tr
                                    v-for="r in vq3.data"
                                    :key="r.id"
                                    class="border-t border-white/[0.03] hover:bg-white/[0.02]"
                                >
                                    <td class="px-2 py-1 text-neutral-500 whitespace-nowrap">{{ formatDate(r.date_set) }}</td>
                                    <td class="px-2 py-1">
                                        <button
                                            class="flex items-center gap-1.5 max-w-[14rem] min-w-0 hover:underline disabled:cursor-default disabled:no-underline text-left"
                                            :disabled="!r.mdd_id"
                                            @click="openProfile(r.mdd_id)"
                                        >
                                            <img
                                                v-if="flagUrl(r.country)"
                                                :src="flagUrl(r.country)!"
                                                :alt="r.country || ''"
                                                class="w-3 h-2 rounded flex-shrink-0"
                                                @error="($event.target as HTMLImageElement).style.display='none'"
                                            />
                                            <span
                                                class="truncate text-neutral-200"
                                                :title="stripColors(r.name)"
                                                v-html="q3ToHtml(r.name)"
                                            ></span>
                                        </button>
                                    </td>
                                    <td class="px-2 py-1 text-right text-emerald-300 font-mono">{{ formatTime(r.time) }}</td>
                                    <td class="px-2 py-1 text-right text-neutral-400">{{ r.rank ?? '-' }}</td>
                                    <td class="px-2 py-1">
                                        <button
                                            class="text-brand-400 hover:underline truncate max-w-[10rem]"
                                            @click="openMap(r.mapname)"
                                        >{{ r.mapname }}</button>
                                    </td>
                                </tr>
                            </tbody>
                        </table>
                    </div>
                    <footer
                        v-if="vq3 && vq3.last_page > 1"
                        class="px-3 py-1.5 border-t border-white/10 flex items-center justify-between text-xs"
                    >
                        <button
                            class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-30"
                            :disabled="vq3.current_page <= 1 || vq3Loading"
                            @click="setVq3Page(vq3!.current_page - 1)"
                        >← Prev</button>
                        <span class="text-neutral-500">page {{ vq3.current_page }}</span>
                        <button
                            class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-30"
                            :disabled="vq3.current_page >= vq3.last_page || vq3Loading"
                            @click="setVq3Page(vq3!.current_page + 1)"
                        >Next →</button>
                    </footer>
                </section>

                <!-- CPM table -->
                <section class="bg-neutral-900/40 border border-white/10 rounded-lg flex flex-col min-h-0">
                    <header class="px-3 py-2 border-b border-white/10 flex items-center justify-between">
                        <div class="text-sm font-semibold text-neutral-200">CPM</div>
                        <div class="text-xs text-neutral-500" v-if="cpm">
                            page {{ cpm.current_page }} / {{ cpm.last_page }} · {{ cpm.total }} total
                        </div>
                    </header>
                    <div class="flex-1 overflow-auto">
                        <div v-if="cpmLoading && !cpm" class="p-6 text-center text-xs text-neutral-500">
                            Loading…
                        </div>
                        <table v-else-if="cpm" class="w-full text-xs">
                            <thead class="text-[10px] uppercase text-neutral-500 sticky top-0 bg-neutral-900/95 backdrop-blur">
                                <tr>
                                    <th class="text-left px-2 py-1.5 font-normal">Date</th>
                                    <th class="text-left px-2 py-1.5 font-normal">Player</th>
                                    <th class="text-right px-2 py-1.5 font-normal">Time</th>
                                    <th class="text-right px-2 py-1.5 font-normal">Rank</th>
                                    <th class="text-left px-2 py-1.5 font-normal">Map</th>
                                </tr>
                            </thead>
                            <tbody>
                                <tr
                                    v-for="r in cpm.data"
                                    :key="r.id"
                                    class="border-t border-white/[0.03] hover:bg-white/[0.02]"
                                >
                                    <td class="px-2 py-1 text-neutral-500 whitespace-nowrap">{{ formatDate(r.date_set) }}</td>
                                    <td class="px-2 py-1">
                                        <button
                                            class="flex items-center gap-1.5 max-w-[14rem] min-w-0 hover:underline disabled:cursor-default disabled:no-underline text-left"
                                            :disabled="!r.mdd_id"
                                            @click="openProfile(r.mdd_id)"
                                        >
                                            <img
                                                v-if="flagUrl(r.country)"
                                                :src="flagUrl(r.country)!"
                                                :alt="r.country || ''"
                                                class="w-3 h-2 rounded flex-shrink-0"
                                                @error="($event.target as HTMLImageElement).style.display='none'"
                                            />
                                            <span
                                                class="truncate text-neutral-200"
                                                :title="stripColors(r.name)"
                                                v-html="q3ToHtml(r.name)"
                                            ></span>
                                        </button>
                                    </td>
                                    <td class="px-2 py-1 text-right text-emerald-300 font-mono">{{ formatTime(r.time) }}</td>
                                    <td class="px-2 py-1 text-right text-neutral-400">{{ r.rank ?? '-' }}</td>
                                    <td class="px-2 py-1">
                                        <button
                                            class="text-brand-400 hover:underline truncate max-w-[10rem]"
                                            @click="openMap(r.mapname)"
                                        >{{ r.mapname }}</button>
                                    </td>
                                </tr>
                            </tbody>
                        </table>
                    </div>
                    <footer
                        v-if="cpm && cpm.last_page > 1"
                        class="px-3 py-1.5 border-t border-white/10 flex items-center justify-between text-xs"
                    >
                        <button
                            class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-30"
                            :disabled="cpm.current_page <= 1 || cpmLoading"
                            @click="setCpmPage(cpm!.current_page - 1)"
                        >← Prev</button>
                        <span class="text-neutral-500">page {{ cpm.current_page }}</span>
                        <button
                            class="px-2 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300 disabled:opacity-30"
                            :disabled="cpm.current_page >= cpm.last_page || cpmLoading"
                            @click="setCpmPage(cpm!.current_page + 1)"
                        >Next →</button>
                    </footer>
                </section>
            </div>
        </template>
    </div>
</template>
