<script setup lang="ts">
    import { computed, onMounted, ref } from 'vue';
    import { useRouter, useRoute } from 'vue-router';
    import { tauri } from './lib/tauri';
    import { useConfigStore } from './stores/config';

    const router = useRouter();
    const route = useRoute();
    const config = useConfigStore();

    // Top nav is hidden during the bootstrap flows (onboarding,
    // version-mismatch screen) - those are full-screen forms that
    // shouldn't be navigable away from. Visible everywhere else so
    // the Play button + tabs are always one click from any view.
    const showNav = computed(() => {
        const r = route.name;
        return r === 'dashboard' || r === 'servers' || r === 'settings';
    });

    const launching = ref(false);
    const launchError = ref<string | null>(null);
    const launchGame = async () => {
        launchError.value = null;
        launching.value = true;
        try {
            await tauri.launchEngine();
        } catch (e: any) {
            launchError.value = e?.toString?.() ?? 'Failed to launch';
        } finally {
            launching.value = false;
        }
    };

    const dismissLaunchError = () => { launchError.value = null; };

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
        <!-- Top nav: tabs on the left, Play CTA on the right. Sticky to
             the top of the window so it never scrolls out of view, and
             the Play button stays one click away from any tab. -->
        <nav
            v-if="showNav"
            class="flex items-center justify-between border-b border-white/10 bg-neutral-950 px-3 h-11 flex-shrink-0"
        >
            <div class="flex items-center gap-1">
                <RouterLink
                    :to="{ name: 'dashboard' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'dashboard'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >Demos</RouterLink>
                <RouterLink
                    :to="{ name: 'servers' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'servers'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >Servers</RouterLink>
                <RouterLink
                    :to="{ name: 'history' }"
                    class="px-3 py-1.5 text-sm rounded transition-colors"
                    :class="route.name === 'history'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >History</RouterLink>
            </div>

            <div class="flex items-center gap-2">
                <!-- Play CTA. Big, green, labelled - this is the "I
                     want to launch the game right now" button. Disabled
                     with a tooltip when the engine path isn't set so
                     the user knows where to go fix it. -->
                <button
                    class="px-3 py-1.5 rounded text-sm font-semibold flex items-center gap-1.5 bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 disabled:opacity-40 disabled:cursor-not-allowed"
                    :disabled="!config.config.engine_path || launching"
                    :title="!config.config.engine_path
                        ? 'Pick an engine in Settings first'
                        : `Quick launch ${config.config.engine_path}`"
                    @click="launchGame"
                >
                    <span>▶</span>
                    <span>{{ launching ? 'Launching…' : 'Quick launch' }}</span>
                </button>

                <RouterLink
                    :to="{ name: 'settings' }"
                    class="px-3 py-1.5 rounded text-sm transition-colors"
                    :class="route.name === 'settings'
                        ? 'bg-white/10 text-neutral-100 font-semibold'
                        : 'text-neutral-400 hover:text-neutral-200 hover:bg-white/5'"
                >Settings</RouterLink>
            </div>
        </nav>

        <p
            v-if="launchError"
            class="px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs text-red-300 flex items-center gap-2 flex-shrink-0"
        >
            <span>{{ launchError }}</span>
            <button class="ml-auto text-neutral-400 hover:text-neutral-200" @click="dismissLaunchError">×</button>
        </p>

        <RouterView v-if="config.loaded" />
        <div v-else class="flex-1 flex items-center justify-center text-sm text-neutral-500">
            Loading…
        </div>
    </div>
</template>
