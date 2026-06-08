<!-- Single source of truth for the "get a token" pitch list. Used by the
     Dashboard no-token banner, the Settings token section, and the Onboarding
     skip-token notice so the three can never drift apart again. Renders bare
     <li> rows (a fragment) - each caller wraps them in its own styled <ul>.
     The embedded player is listed first for prominence (it technically works
     without a token, but we keep the token-free list to just defrag:// so the
     green block doesn't pull the eye away from everything a token unlocks).
     History is absent: the History tab is the local defrag:// join log. -->
<template>
    <li>• Play demos instantly in the launcher - embedded player, no separate game launch</li>
    <li>• Auto-backup of recorded demos</li>
    <li>• YouTube renders straight from the Demos list</li>
    <li>• Server browser with your PB and rank per map</li>
    <li>• Records and Maps tabs (times + your rank)</li>
    <li>• Maps tab: online map list with weapon/item/function icons and per-map info</li>
    <li>• Map history - which maps played on the servers you joined</li>
    <li>• Record + system notifications from defrag.racing</li>
</template>
