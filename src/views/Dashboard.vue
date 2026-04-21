<script setup lang="ts">
    import { onMounted, onUnmounted, ref } from 'vue';
    import { useRouter } from 'vue-router';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { tauri, type UploadStateSnapshot, type PendingUpload } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';

    const router = useRouter();
    const config = useConfigStore();

    const queue = ref<UploadStateSnapshot>({ items: [] });
    const toggling = ref(false);
    const toggleError = ref<string | null>(null);

    let unlisten: UnlistenFn | null = null;

    onMounted(async () => {
        queue.value = await tauri.getUploadState();
        unlisten = await listen<UploadStateSnapshot>('upload_state_changed', (ev) => {
            queue.value = ev.payload;
        });
    });

    onUnmounted(() => {
        if (unlisten) unlisten();
    });

    const toggle = async () => {
        toggleError.value = null;
        toggling.value = true;
        try {
            if (config.autoUploadRunning) {
                await tauri.stopAutoUpload();
            } else {
                await tauri.startAutoUpload();
            }
            await config.refresh();
        } catch (e: any) {
            toggleError.value = e.toString();
        } finally {
            toggling.value = false;
        }
    };

    const statusLabel = (item: PendingUpload) => {
        switch (item.status) {
            case 'pending': return 'Waiting';
            case 'hashing': return 'Hashing';
            case 'uploading': return 'Uploading';
            case 'done': return 'Uploaded';
            case 'duplicate': return 'Already backed up';
            case 'error': return 'Error';
        }
    };

    const statusColor = (item: PendingUpload) => {
        switch (item.status) {
            case 'done': return 'text-emerald-400';
            case 'duplicate': return 'text-cyan-400';
            case 'error': return 'text-red-400';
            case 'uploading':
            case 'hashing': return 'text-brand-400';
            default: return 'text-neutral-500';
        }
    };
</script>

<template>
    <div class="flex-1 flex flex-col">
        <!-- top bar -->
        <header class="px-5 py-3 border-b border-white/10 flex items-center justify-between">
            <div class="flex items-center gap-2">
                <div class="w-2 h-2 rounded-full" :class="config.autoUploadRunning ? 'bg-emerald-400' : 'bg-neutral-600'"></div>
                <div class="text-sm">
                    <span class="font-semibold">Auto-upload</span>
                    <span class="text-neutral-500 ml-1">{{ config.autoUploadRunning ? 'running' : 'off' }}</span>
                </div>
            </div>
            <div class="flex items-center gap-2">
                <button
                    class="px-3 py-1.5 rounded text-sm font-semibold"
                    :class="config.autoUploadRunning
                        ? 'bg-white/5 hover:bg-white/10 text-neutral-200'
                        : 'bg-brand-500/20 hover:bg-brand-500/30 text-brand-400'"
                    :disabled="toggling"
                    @click="toggle"
                >
                    {{ config.autoUploadRunning ? 'Stop' : 'Start' }}
                </button>
                <button
                    class="px-3 py-1.5 rounded bg-white/5 hover:bg-white/10 text-sm text-neutral-300"
                    @click="router.push({ name: 'settings' })"
                >Settings</button>
            </div>
        </header>

        <p v-if="toggleError" class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300">
            {{ toggleError }}
        </p>

        <!-- body -->
        <div class="flex-1 overflow-auto">
            <div v-if="!queue.items.length" class="h-full flex items-center justify-center p-8">
                <div class="text-center space-y-2 max-w-sm">
                    <div class="text-5xl">🎬</div>
                    <div class="text-neutral-300 font-semibold">No demos yet</div>
                    <p class="text-sm text-neutral-500">
                        <template v-if="config.autoUploadRunning">
                            The launcher is watching your demos folder. Record a run and it will appear here.
                        </template>
                        <template v-else>
                            Turn on auto-upload to start watching your demos folder. New demos will appear here as they are backed up.
                        </template>
                    </p>
                </div>
            </div>

            <ul v-else class="divide-y divide-white/[0.04]">
                <li v-for="item in queue.items" :key="item.path" class="px-5 py-3 flex items-center gap-3">
                    <div class="flex-1 min-w-0">
                        <div class="text-sm text-neutral-100 truncate">{{ item.filename }}</div>
                        <div class="text-xs text-neutral-500 truncate">{{ item.path }}</div>
                    </div>
                    <div class="text-xs font-semibold" :class="statusColor(item)">
                        {{ statusLabel(item) }}
                    </div>
                </li>
            </ul>
        </div>
    </div>
</template>
