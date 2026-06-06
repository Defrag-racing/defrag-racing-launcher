<!-- Single source of truth for "what needs a defrag.racing token".
     Used by the Dashboard no-token banner, the Settings token section, and
     the Onboarding skip-token notice so the three can never drift apart
     again. Renders bare <li> rows (a fragment) - each caller wraps them in
     its own styled <ul>. History is deliberately absent: the History tab is
     the local defrag:// join log and works without a token. -->
<template>
    <li>• Auto-backup of recorded demos</li>
    <li>• YouTube renders straight from the Demos list</li>
    <li>• Server browser with your PB and rank per map</li>
    <li>• Records and Maps tabs (times + your rank)</li>
    <li>• Maps tab: weapon/item/function icons + "Run offline" (auto-downloads any map and plays it in VQ3/CPM)</li>
    <li>• Map history - which maps played on the servers you joined</li>
    <li>• Record + system notifications from defrag.racing</li>
</template>
