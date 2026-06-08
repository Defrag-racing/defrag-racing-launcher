<script setup lang="ts">
    // Player tab: browse every playable .dm_68 (the engine install's
    // defrag/demos folder) and play it embedded. The playback UI + engine I/O
    // live in DemoPlayerPanel; this view is just the picker + the panel.

    import { onActivated, onMounted, ref } from 'vue';
    import { tauri, type PlayerDemo } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';
    import DemoPlayerPanel, { type PlayTarget } from '../components/DemoPlayerPanel.vue';

    const config = useConfigStore();
    const isWindows = navigator.userAgent.includes('Windows');

    const demos = ref<PlayerDemo[]>([]);
    const loadingList = ref(false);
    const listError = ref<string | null>(null);
    const search = ref('');
    const selected = ref<PlayTarget | null>(null);

    const loadDemos = async () => {
        if (!config.config.engine_path || !isWindows) {
            demos.value = [];
            return;
        }
        loadingList.value = true;
        listError.value = null;
        try {
            demos.value = await tauri.listPlayerDemos();
        } catch (e: any) {
            listError.value = e?.toString?.() ?? 'Failed to list demos';
            demos.value = [];
        } finally {
            loadingList.value = false;
        }
    };

    const filteredDemos = () => {
        const q = search.value.trim().toLowerCase();
        if (!q) return demos.value;
        return demos.value.filter((d) => d.name.toLowerCase().includes(q));
    };

    const formatDemoName = (name: string): string => {
        const n = name.replace(/\.dm_68$/i, '');
        const m = n.match(/^(.+?)\[([^\]]+)\](\d{2})\.(\d{2})\.(\d{3})\(([^.]+)\.([^)]+)\)/);
        if (m) return `${m[1]}   ${m[2]}   ${m[3]}:${m[4]}.${m[5]}   ${m[6]} (${m[7]})`;
        return n;
    };
    const fmtSize = (b: number) => {
        if (b < 1024) return `${b} B`;
        if (b < 1024 * 1024) return `${(b / 1024).toFixed(0)} KB`;
        return `${(b / 1024 / 1024).toFixed(1)} MB`;
    };

    const pick = (d: PlayerDemo) => {
        selected.value = { path: d.path, name: d.name };
    };

    onMounted(loadDemos);
    onActivated(loadDemos);
</script>

<template>
    <div class="flex flex-col h-full bg-neutral-950 text-neutral-200">
        <div v-if="!isWindows" class="m-4 p-3 rounded bg-amber-500/10 border border-amber-500/30 text-amber-300 text-sm">
            The embedded demo player is only available on Windows.
        </div>
        <div
            v-else-if="!config.config.engine_path"
            class="m-4 p-3 rounded bg-amber-500/10 border border-amber-500/30 text-amber-300 text-sm"
        >
            Pick your Defrag engine in Settings first - the player needs it to find your
            <span class="font-mono">defrag/demos</span> folder.
        </div>

        <template v-else>
            <!-- Player panel (idle prompt until a demo is picked) -->
            <div class="flex-1 min-h-0">
                <DemoPlayerPanel :demo="selected" @close="selected = null" />
            </div>

            <!-- Demo picker -->
            <div class="flex-shrink-0 border-t border-white/10 bg-neutral-950 max-h-[38%] flex flex-col">
                <div class="flex items-center gap-2 px-3 py-2 border-b border-white/5">
                    <input
                        v-model="search"
                        type="text"
                        placeholder="Filter demos…"
                        class="flex-1 px-2 py-1 rounded bg-white/5 border border-white/10 text-sm focus:outline-none focus:border-brand-500"
                    />
                    <button
                        class="px-2 py-1 rounded text-sm bg-white/5 hover:bg-white/10 text-neutral-300"
                        :disabled="loadingList"
                        @click="loadDemos"
                    >{{ loadingList ? '…' : '↻ Refresh' }}</button>
                </div>

                <div v-if="listError" class="px-3 py-2 text-sm text-red-400">{{ listError }}</div>
                <div v-else-if="!loadingList && demos.length === 0" class="px-3 py-6 text-center text-sm text-neutral-500">
                    No demos found under your engine's <span class="font-mono">defrag/demos</span> folder.
                </div>

                <div class="overflow-y-auto">
                    <button
                        v-for="d in filteredDemos()"
                        :key="d.rel"
                        class="w-full flex items-center gap-3 px-3 py-2 text-left border-b border-white/5 hover:bg-white/5 transition-colors"
                        :class="selected?.path === d.path ? 'bg-brand-500/10' : ''"
                        @click="pick(d)"
                    >
                        <span class="text-neutral-500">{{ selected?.path === d.path ? '▶' : '▷' }}</span>
                        <span class="flex-1 truncate text-sm">{{ formatDemoName(d.name) }}</span>
                        <span class="text-xs text-neutral-500 tabular-nums">{{ fmtSize(d.size) }}</span>
                    </button>
                </div>
            </div>
        </template>
    </div>
</template>
