<script setup lang="ts">
    // Embedded demo player (Windows only). Plays a local .dm_68 Defrag demo
    // inside the launcher: the backend spawns a bundled oDFe engine as a native
    // child window placed over the black "stage" region below, and drives it
    // over a loopback control channel. This view owns the picker + transport UI
    // and the playhead; all engine I/O is in src-tauri/src/demo_player.rs.
    //
    // The render area keeps the demo's real aspect (from engine_demo_resolution)
    // so the defrag HUD/FOV aren't distorted - the backend letterboxes the stage
    // inside the region and the leftover margins read as black bars.

    import { onActivated, onDeactivated, onMounted, onUnmounted, ref } from 'vue';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { getCurrentWindow } from '@tauri-apps/api/window';
    import { tauri, type PlayerDemo, type DemoPlayerStatus } from '../lib/tauri';
    import { useConfigStore } from '../stores/config';

    const config = useConfigStore();

    const isWindows = navigator.userAgent.includes('Windows');

    const demos = ref<PlayerDemo[]>([]);
    const loadingList = ref(false);
    const listError = ref<string | null>(null);
    const search = ref('');

    const selected = ref<PlayerDemo | null>(null);
    const playing = ref(false);
    const starting = ref(false);
    const playError = ref<string | null>(null);

    // Playhead state (ms), fed by demo-player-status events.
    const posMs = ref(0);
    const lenMs = ref(0);
    const paused = ref(false);
    const speed = ref(1);

    // Scrub display in seconds.
    const posSec = ref(0);
    const lenSec = ref(0);

    // Interaction guards (mirrors the validated test harness behaviour).
    let dragging = false;
    let seekHoldUntil = 0; // wall-clock ms until which status lines don't move the slider
    let measured = false; // learned the real length yet?
    let measureAttemptAt = 0;
    let seekTarget = 0; // running ms target for arrow-key seeking
    let lastArrowAt = -10000;

    const embedRegion = ref<HTMLDivElement | null>(null);
    let unlisten: UnlistenFn | null = null;
    let unlistenMoved: UnlistenFn | null = null;
    let resizeObs: ResizeObserver | null = null;
    let resizeTimer: number | null = null;
    let moveRaf: number | null = null;
    let lastAspect = 16 / 9;

    const now = () => Date.now();

    // ---- demo list ---------------------------------------------------------

    const loadDemos = async () => {
        if (!config.config.engine_path || !isWindows) {
            demos.value = [];
            return;
        }
        loadingList.value = true;
        listError.value = null;
        try {
            demos.value = await tauri.listPlayerDemos();
        } catch (e: any) {
            listError.value = e?.toString?.() ?? 'Failed to list demos';
            demos.value = [];
        } finally {
            loadingList.value = false;
        }
    };

    const filteredDemos = () => {
        const q = search.value.trim().toLowerCase();
        if (!q) return demos.value;
        return demos.value.filter((d) => d.name.toLowerCase().includes(q));
    };

    // Parse "map[physics]MM.SS.mmm(player.country).dm_68" into a readable line.
    const formatDemoName = (name: string): string => {
        const n = name.replace(/\.dm_68$/i, '');
        const m = n.match(
            /^(.+?)\[([^\]]+)\](\d{2})\.(\d{2})\.(\d{3})\(([^.]+)\.([^)]+)\)/,
        );
        if (m) {
            return `${m[1]}   ${m[2]}   ${m[3]}:${m[4]}.${m[5]}   ${m[6]} (${m[7]})`;
        }
        return n;
    };

    // ---- region measurement ------------------------------------------------

    const computeRegion = () => {
        const el = embedRegion.value;
        if (!el) return null;
        const r = el.getBoundingClientRect();
        const dpr = window.devicePixelRatio || 1;
        return {
            x: Math.round(r.left * dpr),
            y: Math.round(r.top * dpr),
            w: Math.round(r.width * dpr),
            h: Math.round(r.height * dpr),
        };
    };

    // ---- playback ----------------------------------------------------------

    const play = async (demo: PlayerDemo) => {
        if (!isWindows) {
            playError.value = 'The embedded demo player is only available on Windows.';
            return;
        }
        playError.value = null;
        starting.value = true;
        // Stop any current session first.
        if (playing.value) {
            try {
                await tauri.demoPlayerStop();
            } catch { /* ignore */ }
            playing.value = false;
        }
        selected.value = demo;
        posMs.value = 0;
        lenMs.value = 0;
        posSec.value = 0;
        lenSec.value = 0;
        paused.value = false;
        speed.value = 1;
        measured = false;
        measureAttemptAt = 0;
        seekTarget = 0;
        seekHoldUntil = 0;

        try {
            // Resolution/aspect from the user's video cvars (desktop res for r_mode -2).
            const dpr = window.devicePixelRatio || 1;
            const dw = Math.round(window.screen.width * dpr);
            const dh = Math.round(window.screen.height * dpr);
            const rt = await tauri.engineDemoResolution(dw, dh).catch(() => null);
            lastAspect = rt && rt.aspect > 0 ? rt.aspect : 16 / 9;

            const region = computeRegion();
            if (!region) throw new Error('Render area not ready');
            await tauri.demoPlayerStart(demo.rel, region, lastAspect);
            playing.value = true;
        } catch (e: any) {
            playError.value = e?.toString?.() ?? 'Failed to start playback';
            selected.value = null;
        } finally {
            starting.value = false;
        }
    };

    const stop = async () => {
        if (!playing.value) return;
        try {
            await tauri.demoPlayerStop();
        } catch { /* ignore */ }
        playing.value = false;
        selected.value = null;
    };

    const cmd = (line: string) => {
        tauri.demoPlayerCommand(line).catch(() => { /* best effort */ });
    };

    // ---- transport ---------------------------------------------------------

    const doPause = () => {
        cmd('demopause 1');
        paused.value = true;
    };
    const setSpeed = (x: number) => {
        cmd(`demopause 0; timescale ${x}`);
        paused.value = false;
        speed.value = x;
    };

    const SPEEDS = [0.1, 0.25, 0.5, 1, 2, 4, 8];

    // ---- scrubbing ---------------------------------------------------------

    const onScrubInput = (e: Event) => {
        dragging = true;
        const v = Number((e.target as HTMLInputElement).value);
        posSec.value = v;
    };
    const onScrubChange = (e: Event) => {
        const v = Number((e.target as HTMLInputElement).value);
        posSec.value = v;
        cmd(`seekdemo ${v * 1000}`);
        dragging = false;
        // Hold the live updater off so stale "old position" status lines don't
        // yank the slider back before the engine applies the seek.
        seekHoldUntil = now() + 700;
    };

    // Arrow keys: tap = +/-5 s from the live position; hold (auto-repeat) =
    // smooth +/-2 s throttled. Mirrors the harness.
    const onKeydown = (e: KeyboardEvent) => {
        if (!playing.value) return;
        let dir = 0;
        if (e.key === 'ArrowRight') dir = 1;
        else if (e.key === 'ArrowLeft') dir = -1;
        if (dir === 0) return;
        e.preventDefault();
        const t = now();
        const gap = t - lastArrowAt;
        if (gap > 350) {
            seekTarget = posMs.value + dir * 5000;
        } else {
            if (gap < 90) return;
            seekTarget = seekTarget + dir * 2000;
        }
        lastArrowAt = t;
        if (seekTarget < 0) seekTarget = 0;
        if (lenMs.value > 0 && seekTarget > lenMs.value) seekTarget = lenMs.value;
        cmd(`seekdemo ${Math.round(seekTarget)}`);
        const sec = Math.round(seekTarget / 1000);
        posSec.value = Math.min(Math.max(sec, 0), lenSec.value || sec);
        seekHoldUntil = t + 300;
    };

    // ---- status events -----------------------------------------------------

    const onStatus = (s: DemoPlayerStatus) => {
        const start = s.start;
        posMs.value = Math.max(0, s.time - start);
        lenMs.value = s.total > start ? s.total - start : 0;
        paused.value = s.paused;

        // One-time length measurement: the engine only knows the length once it
        // has read to EOF, so right after connecting we seek far past the end
        // (engine clamps + reports the real total) then jump back to the start.
        if (!measured) {
            if (lenMs.value > 0) {
                lenSec.value = Math.max(1, Math.round(lenMs.value / 1000));
                cmd('seekdemo 0');
                measured = true;
                seekHoldUntil = now() + 500;
            } else if (now() - measureAttemptAt > 1200) {
                measureAttemptAt = now();
                cmd('seekdemo 86400000'); // 24h -> forces read to EOF
            }
            return;
        }

        if (lenMs.value > 0) {
            const maxSec = Math.round(lenMs.value / 1000);
            if (maxSec > 0) lenSec.value = maxSec;
        }
        if (dragging || now() < seekHoldUntil) return;
        posSec.value = Math.min(Math.max(Math.round(posMs.value / 1000), 0), lenSec.value || 0);
    };

    // ---- resize ------------------------------------------------------------

    const onRegionResize = () => {
        if (!playing.value) return;
        if (resizeTimer !== null) window.clearTimeout(resizeTimer);
        // Debounce: a drag-resize only re-inits the engine once it settles.
        resizeTimer = window.setTimeout(() => {
            const region = computeRegion();
            if (region) tauri.demoPlayerSetRegion(region, lastAspect).catch(() => {});
        }, 350);
    };

    // The render window is a separate top-level overlay, so it doesn't move with
    // the launcher automatically - when the user drags the window, reposition it
    // (cheap, no engine re-init). Coalesced to one call per frame so a drag
    // doesn't flood the IPC.
    const onWindowMoved = () => {
        if (!playing.value) return;
        if (moveRaf !== null) return;
        moveRaf = window.requestAnimationFrame(() => {
            moveRaf = null;
            const region = computeRegion();
            if (region) tauri.demoPlayerReposition(region, lastAspect).catch(() => {});
        });
    };

    // ---- formatting --------------------------------------------------------

    const fmt = (sec: number) => {
        const m = Math.floor(sec / 60);
        const s = Math.floor(sec % 60);
        return `${m}:${s.toString().padStart(2, '0')}`;
    };
    const fmtSize = (b: number) => {
        if (b < 1024) return `${b} B`;
        if (b < 1024 * 1024) return `${(b / 1024).toFixed(0)} KB`;
        return `${(b / 1024 / 1024).toFixed(1)} MB`;
    };

    // ---- lifecycle ---------------------------------------------------------

    onMounted(async () => {
        unlisten = await listen<DemoPlayerStatus>('demo-player-status', (e) => onStatus(e.payload));
        // Follow the launcher when it's dragged (overlay is a separate window).
        unlistenMoved = await getCurrentWindow().onMoved(onWindowMoved);
        window.addEventListener('keydown', onKeydown);
        if (embedRegion.value) {
            resizeObs = new ResizeObserver(onRegionResize);
            resizeObs.observe(embedRegion.value);
        }
        await loadDemos();
    });

    onActivated(() => {
        loadDemos();
    });

    onDeactivated(() => {
        stop();
    });

    onUnmounted(() => {
        stop();
        if (unlisten) unlisten();
        if (unlistenMoved) unlistenMoved();
        window.removeEventListener('keydown', onKeydown);
        if (resizeObs) resizeObs.disconnect();
        if (resizeTimer !== null) window.clearTimeout(resizeTimer);
        if (moveRaf !== null) window.cancelAnimationFrame(moveRaf);
    });
</script>

<template>
    <div class="flex flex-col h-full bg-neutral-950 text-neutral-200">
        <!-- Not-Windows / no-engine notices -->
        <div v-if="!isWindows" class="m-4 p-3 rounded bg-amber-500/10 border border-amber-500/30 text-amber-300 text-sm">
            The embedded demo player is only available on Windows.
        </div>
        <div
            v-else-if="!config.config.engine_path"
            class="m-4 p-3 rounded bg-amber-500/10 border border-amber-500/30 text-amber-300 text-sm"
        >
            Pick your Defrag engine in Settings first - the player needs it to find your
            <span class="font-mono">defrag/demos</span> folder.
        </div>

        <template v-else>
            <!-- Demo name bar. Kept ABOVE the render area (not overlaid) so the
                 native engine window, which sits on top of the render region,
                 doesn't cover it. -->
            <div
                v-if="playing && selected"
                class="flex-shrink-0 px-4 py-2 bg-neutral-900 border-b border-white/10 text-center text-sm font-semibold text-neutral-100 truncate"
            >{{ formatDemoName(selected.name) }}</div>

            <!-- Stage: black render region the engine draws into (aspect-correct,
                 letterboxed by the backend). The native child window covers this. -->
            <div class="flex-1 min-h-0 relative bg-black">
                <div ref="embedRegion" class="absolute inset-0"></div>

                <!-- Overlay shown only when nothing is playing (no native window
                     covering it then). -->
                <div
                    v-if="!playing"
                    class="absolute inset-0 flex flex-col items-center justify-center text-neutral-500 pointer-events-none"
                >
                    <div class="text-5xl mb-3">▶</div>
                    <div class="text-sm">Pick a demo below to play it here</div>
                </div>
            </div>

            <!-- Errors live below the render area so the native window can't hide them. -->
            <div
                v-if="playError"
                class="flex-shrink-0 px-3 py-2 bg-red-500/15 border-t border-red-500/30 text-red-300 text-xs"
            >{{ playError }}</div>

            <!-- Transport bar -->
            <div v-if="playing" class="flex-shrink-0 border-t border-white/10 bg-neutral-900 px-3 py-2">
                <div class="flex items-center gap-2">
                    <button
                        class="px-3 py-1.5 rounded text-sm font-semibold"
                        :class="paused ? 'bg-emerald-500/30 text-emerald-200' : 'bg-white/5 hover:bg-white/10 text-neutral-200'"
                        @click="doPause"
                    >⏸ Pause</button>
                    <button
                        v-for="x in SPEEDS"
                        :key="x"
                        class="px-2.5 py-1.5 rounded text-sm font-semibold"
                        :class="!paused && speed === x ? 'bg-brand-500/30 text-brand-200' : 'bg-white/5 hover:bg-white/10 text-neutral-300'"
                        @click="setSpeed(x)"
                    >{{ x === 1 ? '1x (Play)' : x + 'x' }}</button>

                    <input
                        type="range"
                        class="flex-1 mx-2 accent-brand-500"
                        min="0"
                        :max="lenSec || 1"
                        step="1"
                        :value="posSec"
                        @input="onScrubInput"
                        @change="onScrubChange"
                    />
                    <span class="font-mono text-sm tabular-nums w-24 text-right">
                        {{ fmt(posSec) }} / {{ fmt(lenSec) }}
                    </span>
                    <button
                        class="ml-1 px-3 py-1.5 rounded text-sm font-semibold bg-red-500/20 hover:bg-red-500/30 text-red-300"
                        @click="stop"
                    >Stop</button>
                </div>
                <div class="mt-1 text-[11px] text-neutral-500">
                    Tip: ← / → seek 5 s (hold to scrub). Resize the window to rescale.
                </div>
            </div>

            <!-- Demo picker -->
            <div class="flex-shrink-0 border-t border-white/10 bg-neutral-950 max-h-[40%] flex flex-col">
                <div class="flex items-center gap-2 px-3 py-2 border-b border-white/5">
                    <input
                        v-model="search"
                        type="text"
                        placeholder="Filter demos…"
                        class="flex-1 px-2 py-1 rounded bg-white/5 border border-white/10 text-sm focus:outline-none focus:border-brand-500"
                    />
                    <button
                        class="px-2 py-1 rounded text-sm bg-white/5 hover:bg-white/10 text-neutral-300"
                        :disabled="loadingList"
                        @click="loadDemos"
                    >{{ loadingList ? '…' : '↻ Refresh' }}</button>
                </div>

                <div v-if="listError" class="px-3 py-2 text-sm text-red-400">{{ listError }}</div>
                <div v-else-if="!loadingList && demos.length === 0" class="px-3 py-6 text-center text-sm text-neutral-500">
                    No demos found under your engine's <span class="font-mono">defrag/demos</span> folder.
                </div>

                <div class="overflow-y-auto">
                    <button
                        v-for="d in filteredDemos()"
                        :key="d.rel"
                        class="w-full flex items-center gap-3 px-3 py-2 text-left border-b border-white/5 hover:bg-white/5 transition-colors"
                        :class="selected?.rel === d.rel ? 'bg-brand-500/10' : ''"
                        @click="play(d)"
                    >
                        <span class="text-neutral-500">{{ selected?.rel === d.rel && playing ? '▶' : '▷' }}</span>
                        <span class="flex-1 truncate text-sm">{{ formatDemoName(d.name) }}</span>
                        <span class="text-xs text-neutral-500 tabular-nums">{{ fmtSize(d.size) }}</span>
                    </button>
                </div>
            </div>
        </template>
    </div>
</template>
