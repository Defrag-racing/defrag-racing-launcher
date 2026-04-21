<script setup lang="ts">
    import { onMounted } from 'vue';
    import { useRouter } from 'vue-router';
    import { useConfigStore } from './stores/config';

    const router = useRouter();
    const config = useConfigStore();

    onMounted(async () => {
        await config.refresh();
        // First run always takes the user through the onboarding wizard
        // before the real UI opens. They can re-run it later from settings.
        if (! config.config.onboarding_completed) {
            router.replace({ name: 'onboarding' });
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
