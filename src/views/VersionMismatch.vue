<script setup lang="ts">
    // Shown once, on first launch after a version change. Gives the user
    // the choice to wipe the old settings + token (clean slate) or keep
    // them (the default — most users will want this after a normal
    // upgrade). Either path bumps the stored config_version so this
    // screen won't show again until the next upgrade.

    import { ref } from 'vue';
    import { useRouter, useRoute } from 'vue-router';
    import { tauri } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';

    const router = useRouter();
    const route = useRoute();
    const config = useConfigStore();

    const previous = ref<string>(String(route.query.previous ?? 'unknown'));
    const current = ref<string>(String(route.query.current ?? '?'));
    const busy = ref(false);

    const keep = async () => {
        busy.value = true;
        try {
            await tauri.acknowledgeVersion();
            await config.refresh();
            router.replace({ name: 'dashboard' });
        } finally {
            busy.value = false;
        }
    };

    const startFresh = async () => {
        if (! confirm('Clear all settings and the stored token? Demos on your PC are not affected.')) return;
        busy.value = true;
        try {
            await tauri.resetLauncher();
            await config.refresh();
            router.replace({ name: 'onboarding' });
        } finally {
            busy.value = false;
        }
    };
</script>

<template>
    <div class="min-h-full flex items-center justify-center p-6">
        <div class="max-w-md w-full bg-neutral-900 border border-white/10 rounded-xl p-6 space-y-5">
            <div>
                <h1 class="text-xl font-bold">Launcher updated</h1>
                <p class="text-sm text-neutral-400 mt-2 leading-relaxed">
                    You were on version <code class="text-xs bg-black/40 px-1 rounded">{{ previous }}</code>
                    and are now on <code class="text-xs bg-black/40 px-1 rounded">{{ current }}</code>.
                    Do you want to keep your settings or start fresh?
                </p>
            </div>

            <div class="space-y-2">
                <button class="w-full btn-primary" :disabled="busy" @click="keep">
                    Keep settings
                </button>
                <button class="w-full btn-danger" :disabled="busy" @click="startFresh">
                    Start fresh (wipe settings + token)
                </button>
            </div>

            <p class="text-xs text-neutral-500 text-center">
                "Keep settings" is the right choice for a normal upgrade. Pick "Start fresh" only if you want to redo the whole setup.
            </p>
        </div>
    </div>
</template>

<style scoped>
.btn-primary {
    @apply px-4 py-2 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-400 text-sm font-semibold disabled:opacity-40 disabled:cursor-not-allowed;
}
.btn-danger {
    @apply px-4 py-2 rounded bg-red-500/15 hover:bg-red-500/25 text-red-300 text-sm font-semibold disabled:opacity-40 disabled:cursor-not-allowed;
}
</style>
