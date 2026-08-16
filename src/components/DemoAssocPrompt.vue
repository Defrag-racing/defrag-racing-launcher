<script lang="ts">
    // Asked once: should a double-clicked .dm_68 open here?
    //
    // The right-click entry is registered by the installer and re-asserted on
    // every start; becoming the DEFAULT program is the user's call, asked one
    // time and then only in Settings. Most people already have DemoCleaner3 on
    // this file type and nothing here takes it away from them.
    //
    // It lives at App level, next to the update banner, rather than inside the
    // Demos list where it started. There it sat under the backup panel, below
    // the fold on a short window, and it was invisible on every other tab - so
    // a question asked exactly once could be missed exactly once and never come
    // back. Same reasoning as the update banner: something asked once belongs
    // where the eye already goes.
    import { computed, defineComponent, onMounted, ref } from 'vue';
    import { t } from '../lib/i18n';
    import { tauri, type DemoAssocStatus } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';

    export default defineComponent({
        name: 'DemoAssocPrompt',
        setup() {
            const config = useConfigStore();

            const assoc = ref<DemoAssocStatus | null>(null);
            const busy = ref(false);
            const note = ref<string | null>(null);

            onMounted(async () => {
                try {
                    assoc.value = await tauri.demoAssocStatus();
                } catch {
                    /* the prompt just stays away */
                }
            });

            const show = computed(
                () =>
                    !!assoc.value?.supported &&
                    !assoc.value.is_default &&
                    !config.config.demo_assoc_asked,
            );

            const answer = async (makeDefault: boolean) => {
                busy.value = true;
                note.value = null;
                try {
                    if (makeDefault) {
                        assoc.value = await tauri.demoAssocMakeDefault();

                        // Windows keeps its own UserChoice once somebody has
                        // picked a program, and an app may not write it. Say so
                        // plainly instead of leaving a button that looks like it
                        // did nothing.
                        if (!assoc.value.is_default) {
                            note.value = t(
                                'Windows keeps its own choice for this file type. Right-click a .dm_68, choose "Open with" then "Choose another app", pick Defrag Launcher and tick "Always".',
                            );
                        }
                    }

                    await config.save({ demo_assoc_asked: true });
                } catch (e: any) {
                    note.value = e?.toString?.() ?? t('Could not change the file association');
                } finally {
                    busy.value = false;
                }
            };

            return { show, busy, note, answer };
        },
    });
</script>

<template>
    <div v-if="show || note" class="px-4 pt-3">
        <div
            v-if="show"
            class="rounded-lg border border-white/10 bg-white/[0.04] px-4 py-3 text-xs text-neutral-300"
        >
            <div class="font-semibold text-neutral-200 mb-1">
                {{ $t('Open .dm_68 demos in the launcher?') }}
            </div>
            <p class="text-neutral-400 mb-2">
                {{ $t('Right-clicking a demo already offers to play it here. This is about double-clicking one: it would open here instead of in whatever you use now. You can change it in Settings whenever you like.') }}
            </p>
            <div class="flex items-center justify-end gap-2">
                <button
                    class="px-3 py-1 rounded bg-white/5 hover:bg-white/10 text-neutral-300"
                    :disabled="busy"
                    @click="answer(false)"
                >{{ $t('No thanks') }}</button>
                <button
                    class="px-3 py-1 rounded bg-brand-500/20 hover:bg-brand-500/30 text-brand-300 font-semibold"
                    :disabled="busy"
                    @click="answer(true)"
                >{{ $t('Yes, open them here') }}</button>
            </div>
        </div>

        <p v-if="note" class="mt-2 rounded-lg border border-white/10 bg-white/[0.03] px-4 py-2 text-xs text-neutral-400">
            {{ note }}
        </p>
    </div>
</template>
