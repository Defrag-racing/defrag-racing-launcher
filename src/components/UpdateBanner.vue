<script setup lang="ts">
    // Shared auto-update banner. Renders nothing unless the updater store
    // has an update available / downloading / installing / errored, so it
    // can be dropped anywhere safely. Mounted once at App level (visible on
    // every tab) and again inside Settings' "Check now" card - the two
    // placements are mutually exclusive (App hides it on the settings
    // route) so only one ever shows at a time.
    //
    // Owns its own "what's new" expand: clicking View changes fetches the
    // CHANGELOG entries newer than the installed version and renders them
    // inline. State is local to the instance, which is fine - there's only
    // ever one visible.

    import { ref } from 'vue';
    import { getVersion } from '@tauri-apps/api/app';
    import { useUpdaterStore } from '../stores/updater';
    import { fetchChangelogSince, renderMarkdown, type ChangelogSection } from '../lib/changelog';
    import { t } from '../lib/i18n';

    const updater = useUpdaterStore();

    const whatsNewOpen = ref(false);
    const whatsNewLoading = ref(false);
    const whatsNewError = ref<string | null>(null);
    const whatsNewSections = ref<ChangelogSection[]>([]);
    const whatsNewInstalled = ref<string>('');
    const renderedBody = (body: string) => renderMarkdown(body);

    const toggleWhatsNew = async () => {
        if (whatsNewOpen.value) { whatsNewOpen.value = false; return; }
        whatsNewOpen.value = true;
        if (whatsNewSections.value.length > 0) return;
        whatsNewLoading.value = true;
        whatsNewError.value = null;
        try {
            const installed = await getVersion();
            whatsNewInstalled.value = installed;
            whatsNewSections.value = await fetchChangelogSince(installed);
        } catch (e: any) {
            whatsNewError.value = e?.toString?.() ?? t('Failed to load changelog');
        } finally {
            whatsNewLoading.value = false;
        }
    };

    const installUpdate = () => updater.install();
</script>

<template>
    <div
        v-if="updater.state.kind === 'available'"
        class="border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300 flex-shrink-0"
    >
        <div class="px-5 py-2 flex items-center gap-3">
            <span>{{ $t('Update :version is available.', { version: `v${updater.state.version}` }) }}</span>
            <button class="ml-auto px-2 py-0.5 rounded bg-white/5 hover:bg-white/10" @click="toggleWhatsNew">
                {{ whatsNewOpen ? $t('Hide changes') : $t('View changes') }}
            </button>
            <button class="px-2 py-0.5 rounded bg-brand-500/20 hover:bg-brand-500/30 font-semibold" @click="installUpdate">
                {{ $t('Install and restart') }}
            </button>
        </div>
        <div v-if="whatsNewOpen" class="px-5 py-3 border-t border-brand-500/20 bg-black/30 max-h-72 overflow-y-auto">
            <div v-if="whatsNewLoading" class="text-neutral-400">{{ $t('Loading changelog…') }}</div>
            <div v-else-if="whatsNewError" class="text-red-300">{{ whatsNewError }}</div>
            <div v-else-if="whatsNewSections.length === 0" class="text-neutral-400">
                {{ $t('Nothing newer than :version in the changelog yet.', { version: `v${whatsNewInstalled}` }) }}
            </div>
            <div v-else class="space-y-4">
                <section v-for="s in whatsNewSections" :key="s.version">
                    <h3 class="text-sm font-semibold text-brand-200 mb-1">v{{ s.version }}</h3>
                    <div class="text-xs text-neutral-200" v-html="renderedBody(s.body)"></div>
                </section>
            </div>
        </div>
    </div>
    <div v-else-if="updater.state.kind === 'downloading'" class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300 flex-shrink-0">
        {{ $t('Downloading update…') }} {{ updater.state.percent }}%
    </div>
    <div v-else-if="updater.state.kind === 'installing'" class="px-5 py-2 border-b border-brand-500/20 bg-brand-500/10 text-xs text-brand-300 flex-shrink-0">
        {{ $t('Installing… the launcher will restart in a moment.') }}
    </div>
    <div v-else-if="updater.state.kind === 'error'" class="px-5 py-2 border-b border-red-500/20 bg-red-500/10 text-xs text-red-300 flex-shrink-0">
        {{ $t('Update failed:') }} {{ updater.state.message }}
    </div>
</template>
