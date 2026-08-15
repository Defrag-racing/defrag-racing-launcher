<!-- Single source of truth for the "get a token" pitch list. Used by the
     Dashboard no-token banner, the Settings token section, and the Onboarding
     skip-token notice so the three can never drift apart again. Renders bare
     <li> rows (a fragment) - each caller wraps them in its own styled <ul>.
     The embedded player is listed first for prominence (it technically works
     without a token, but we keep the token-free list to just defrag:// so the
     green block doesn't pull the eye away from everything a token unlocks).
     History is absent: the History tab is the local defrag:// join log. -->
<template>
    <li>• {{ $t('Play demos instantly in the launcher - embedded player, no separate game launch') }}</li>
    <li>• {{ $t('Compare two demos side by side - two synced engines, run vs run') }}</li>
    <li>• {{ $t('Auto-backup of recorded demos') }}</li>
    <li>• {{ $t('YouTube renders straight from the Demos list') }}</li>
    <li>• {{ $t('Server browser with your PB and rank per map') }}</li>
    <li>• {{ $t('Records and Maps tabs (times and your rank)') }}</li>
    <li>• {{ $t('Maps tab: online map list with weapon, item and function icons and per-map info') }}</li>
    <li>• {{ $t('Map history - which maps played on the servers you joined') }}</li>
    <li>• {{ $t('Record and system notifications from defrag.racing') }}</li>
</template>
