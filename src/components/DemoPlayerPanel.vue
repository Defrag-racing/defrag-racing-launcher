<script setup lang="ts">
    // Reusable embedded-demo-player panel (Windows only). Renders the name bar,
    // the black render region the bundled oDFe engine draws into (as a native
    // overlay window managed by the backend), and the transport bar. Driven by a
    // `demo` prop: set it to play, clear it / click the close button to stop.
    //
    // Used both by the Player tab (with its own picker) and inline in the Demos
    // view (as a full-section overlay). All engine I/O lives in
    // src-tauri/src/demo_player.rs; this component only measures the render
    // region, forwards transport commands, and renders the playhead.

    import { onActivated, onDeactivated, onMounted, onUnmounted, ref, watch } from 'vue';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { getCurrentWindow } from '@tauri-apps/api/window';
    import { tauri, type DemoPlayerStatus } from '../lib/tauri';

    /** The demo to play: absolute `path` on disk + a `name` for the banner. */
    export interface PlayTarget {
        path: string;
        name: string;
    }

    const props = defineProps<{ demo: PlayTarget | null }>();
    const emit = defineEmits<{ (e: 'close'): void }>();

    const isWindows = navigator.userAgent.includes('Windows');

    const playing = ref(false);
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
    let seekHoldUntil = 0;
    let measured = false;
    let measureAttemptAt = 0;
    let seekTarget = 0;
    let lastArrowAt = -10000;

    const embedRegion = ref<HTMLDivElement | null>(null);
    let unlisten: UnlistenFn | null = null;
    let unlistenClosed: UnlistenFn | null = null;
    let unlistenMoved: UnlistenFn | null = null;
    let resizeObs: ResizeObserver | null = null;
    let resizeTimer: number | null = null;
    let moveRaf: number | null = null;
    let lastAspect = 16 / 9;

    const now = () => Date.now();

    const SPEEDS = [0.1, 0.25, 0.5, 1, 2, 4, 8];

    // Parse "map[physics]MM.SS.mmm(player.country).dm_68" into a readable line.
    const formatDemoName = (name: string): string => {
        const n = name.replace(/\.dm_68$/i, '');
        const m = n.match(/^(.+?)\[([^\]]+)\](\d{2})\.(\d{2})\.(\d{3})\(([^.]+)\.([^)]+)\)/);
        if (m) return `${m[1]}   ${m[2]}   ${m[3]}:${m[4]}.${m[5]}   ${m[6]} (${m[7]})`;
        return n;
    };

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

    const resetPlayhead = () => {
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
    };

    const start = async (target: PlayTarget) => {
        if (!isWindows) {
            playError.value = 'The embedded demo player is only available on Windows.';
            return;
        }
        playError.value = null;
        resetPlayhead();
        try {
            const dpr = window.devicePixelRatio || 1;
            const dw = Math.round(window.screen.width * dpr);
            const dh = Math.round(window.screen.height * dpr);
            const rt = await tauri.engineDemoResolution(dw, dh).catch(() => null);
            lastAspect = rt && rt.aspect > 0 ? rt.aspect : 16 / 9;

            const region = computeRegion();
            if (!region) throw new Error('Render area not ready');
            await tauri.demoPlayerStart(target.path, region, lastAspect);
            playing.value = true;
        } catch (e: any) {
            playError.value = e?.toString?.() ?? 'Failed to start playback';
            playing.value = false;
        }
    };

    const stop = async () => {
        if (!playing.value) return;
        playing.value = false;
        try {
            await tauri.demoPlayerStop();
        } catch { /* ignore */ }
    };

    const cmd = (line: string) => {
        tauri.demoPlayerCommand(line).catch(() => {});
    };

    const close = () => {
        stop();
        emit('close');
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

    const onScrubInput = (e: Event) => {
        dragging = true;
        posSec.value = Number((e.target as HTMLInputElement).value);
    };
    const onScrubChange = (e: Event) => {
        const v = Number((e.target as HTMLInputElement).value);
        posSec.value = v;
        cmd(`seekdemo ${v * 1000}`);
        dragging = false;
        seekHoldUntil = now() + 700;
    };

    // Jump the playhead by a fixed amount (ms). Used by the Up/Down 10 s seek.
    const seekBy = (deltaMs: number) => {
        const base = now() < seekHoldUntil ? seekTarget : posMs.value;
        seekTarget = base + deltaMs;
        if (seekTarget < 0) seekTarget = 0;
        if (lenMs.value > 0 && seekTarget > lenMs.value) seekTarget = lenMs.value;
        cmd(`seekdemo ${Math.round(seekTarget)}`);
        const sec = Math.round(seekTarget / 1000);
        posSec.value = Math.min(Math.max(sec, 0), lenSec.value || sec);
        seekHoldUntil = now() + 300;
    };

    const onKeydown = (e: KeyboardEvent) => {
        if (!playing.value) return;

        // ESC quits the player entirely (same as the close button).
        if (e.key === 'Escape') {
            e.preventDefault();
            close();
            return;
        }

        // Up / Down jump 10 s back/forward.
        if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
            e.preventDefault();
            seekBy(e.key === 'ArrowUp' ? 10000 : -10000);
            return;
        }

        // Left / Right: 5 s nudge, accelerating to a 2 s scrub when held.
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
        // Only the panel that owns the running session reacts - otherwise an
        // idle panel (e.g. the Player tab while the Demos overlay plays) would
        // also run the measurement seeks against the same engine.
        if (!playing.value) return;
        posMs.value = Math.max(0, s.time - s.start);
        lenMs.value = s.total > s.start ? s.total - s.start : 0;
        paused.value = s.paused;

        if (!measured) {
            if (lenMs.value > 0) {
                lenSec.value = Math.max(1, Math.round(lenMs.value / 1000));
                cmd('seekdemo 0');
                measured = true;
                seekHoldUntil = now() + 500;
            } else if (now() - measureAttemptAt > 1200) {
                measureAttemptAt = now();
                cmd('seekdemo 86400000');
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

    // ---- resize / move -----------------------------------------------------

    const onRegionResize = () => {
        if (!playing.value) return;
        if (resizeTimer !== null) window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => {
            const region = computeRegion();
            if (region) tauri.demoPlayerSetRegion(region, lastAspect).catch(() => {});
        }, 350);
    };

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

    // ---- lifecycle ---------------------------------------------------------

    // Restart when the target demo changes; stop when it's cleared.
    watch(
        () => props.demo,
        (d) => {
            if (d) start(d);
            else stop();
        },
    );

    onMounted(async () => {
        unlisten = await listen<DemoPlayerStatus>('demo-player-status', (e) => onStatus(e.payload));
        // The engine went away on its own (demo ended, crash, or the backend
        // killed it because the launcher was sent to the tray). Drop our UI back
        // to the idle state so a stale "playing" panel doesn't linger.
        unlistenClosed = await listen('demo-player-closed', () => {
            if (!playing.value) return;
            playing.value = false;
            emit('close');
        });
        unlistenMoved = await getCurrentWindow().onMoved(onWindowMoved);
        window.addEventListener('keydown', onKeydown);
        if (embedRegion.value) {
            resizeObs = new ResizeObserver(onRegionResize);
            resizeObs.observe(embedRegion.value);
        }
        if (props.demo) start(props.demo);
    });

    // keep-alive: stop the engine when the host view is backgrounded, resume
    // when it returns (the parent keeps the same `demo` prop).
    onActivated(() => {
        if (props.demo && !playing.value) start(props.demo);
    });
    onDeactivated(() => {
        stop();
    });

    onUnmounted(() => {
        stop();
        if (unlisten) unlisten();
        if (unlistenClosed) unlistenClosed();
        if (unlistenMoved) unlistenMoved();
        window.removeEventListener('keydown', onKeydown);
        if (resizeObs) resizeObs.disconnect();
        if (resizeTimer !== null) window.clearTimeout(resizeTimer);
        if (moveRaf !== null) window.cancelAnimationFrame(moveRaf);
    });
</script>

<template>
    <div class="flex flex-col h-full bg-neutral-950 text-neutral-200">
        <!-- Top bar: demo name + close. Kept ABOVE the render area so the
             native overlay window doesn't cover it. -->
        <div class="flex-shrink-0 flex items-center gap-2 px-3 py-2 bg-neutral-900 border-b border-white/10">
            <span class="flex-1 text-center text-sm font-semibold text-neutral-100 truncate">
                {{ demo ? formatDemoName(demo.name) : 'Demo player' }}
            </span>
            <button
                class="flex-shrink-0 px-2 py-1 rounded text-sm bg-white/5 hover:bg-white/10 text-neutral-300"
                title="Close player"
                @click="close"
            >✕</button>
        </div>

        <!-- Stage: black render region the engine draws into. -->
        <div class="flex-1 min-h-0 relative bg-black">
            <div ref="embedRegion" class="absolute inset-0"></div>
            <div
                v-if="!playing"
                class="absolute inset-0 flex flex-col items-center justify-center text-neutral-500 pointer-events-none"
            >
                <div class="text-5xl mb-3">▶</div>
                <div class="text-sm">{{ isWindows ? 'Loading demo…' : 'The demo player is only available on Windows.' }}</div>
            </div>
        </div>

        <div
            v-if="playError"
            class="flex-shrink-0 px-3 py-2 bg-red-500/15 border-t border-red-500/30 text-red-300 text-xs"
        >{{ playError }}</div>

        <!-- Transport bar -->
        <div class="flex-shrink-0 border-t border-white/10 bg-neutral-900 px-3 py-2">
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
            </div>
            <div class="mt-1 text-[11px] text-neutral-500">
                Tip: ← / → seek 5 s (hold to scrub) · ↑ / ↓ seek 10 s · Esc closes the player.
            </div>
        </div>
    </div>
</template>
