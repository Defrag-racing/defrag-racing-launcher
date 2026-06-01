<script setup lang="ts">
    // Compact "where am I watching" chip used in the Demos header.
    // Shows only the leaf of the configured demos folder
    // (`…\defrag\demos`) with the full, prefix-stripped path on hover,
    // plus a button that jumps to Settings and highlights the folder
    // field so the user knows exactly what to change. Replaces the old
    // raw `\\?\E:\…` <code> blob that nobody could read.

    import { computed } from 'vue';
    import { useRouter } from 'vue-router';
    import { useConfigStore } from '../stores/config';
    import { displayPath, folderSnippet } from '../lib/path';

    const router = useRouter();
    const config = useConfigStore();

    const full = computed(() => displayPath(config.config.demos_path));
    const snippet = computed(() => folderSnippet(config.config.demos_path, 2));
    const isSet = computed(() => !!config.config.demos_path);

    const openSettings = () => {
        router.push({ name: 'settings', query: { highlight: 'demos' } });
    };
</script>

<template>
    <div class="flex items-center gap-1.5 text-xs min-w-0">
        <span class="text-neutral-500 flex-shrink-0">📁</span>
        <code
            v-if="isSet"
            class="bg-black/40 px-1.5 py-0.5 rounded text-neutral-300 truncate max-w-[220px]"
            :title="full"
        >{{ snippet }}</code>
        <span v-else class="text-amber-300/90 flex-shrink-0">folder not set</span>
        <button
            class="flex-shrink-0 text-brand-400 hover:text-brand-300 hover:underline"
            :title="isSet ? `Watching ${full} - click to change it in Settings` : 'Set your demos folder in Settings'"
            @click="openSettings"
        >{{ isSet ? 'Change in Settings →' : 'Set folder →' }}</button>
    </div>
</template>
