<script setup lang="ts">
    import { onMounted } from 'vue';
    import { useRouter } from 'vue-router';
    import { tauri } from './lib/tauri';
    import { useConfigStore } from './stores/config';

    const router = useRouter();
    const config = useConfigStore();

    onMounted(async () => {
        await config.refresh();

        // Upgrade-aware boot flow:
        //  1. Fresh install (no onboarding) → onboarding wizard
        //  2. Config left behind by an older launcher → mismatch screen
        //     (user picks keep-or-wipe before the dashboard)
        //  3. Normal same-version boot → dashboard (via the default route)
        if (! config.config.onboarding_completed) {
            router.replace({ name: 'onboarding' });
            return;
        }

        const previous = await tauri.previousVersion();
        if (previous) {
            const current = await tauri.appVersion();
            router.replace({
                name: 'version-mismatch',
                query: { previous, current },
            });
        }
    });
</script>

<template>
    <div class="h-full flex flex-col">
        <RouterView v-if="config.loaded" />
        <div v-else class="flex-1 flex items-center justify-center text-sm text-neutral-500">
            Loading…
        </div>
    </div>
</template>
