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

    import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch } from 'vue';
    import { listen, type UnlistenFn } from '@tauri-apps/api/event';
    import { getCurrentWindow } from '@tauri-apps/api/window';
    import { tauri, type DemoPlayerStatus } from '../lib/tauri';

    /** The demo to play: absolute `path` on disk + a `name` for the banner. */
    export interface PlayTarget {
        path: string;
        name: string;
    }

    /** Two demos to compare side by side (premium feature). */
    export interface CompareTarget {
        a: PlayTarget;
        b: PlayTarget;
    }

    const props = defineProps<{
        demo: PlayTarget | null;
        compare?: CompareTarget | null;
    }>();
    const emit = defineEmits<{ (e: 'close'): void }>();

    // Comparison mode: two engines side by side, driven in lockstep.
    const isCompare = computed(() => !!props.compare);

    // Pane 1 (right, comparison only) mirror of the playhead, for its own timer.
    const pos1Ms = ref(0);
    const len1Ms = ref(0);
    const pos1Sec = ref(0);
    const len1Sec = ref(0);
    let measured1 = false;
    // Sync offset (ms) applied to pane 1 so two runs with different lead-ins line
    // up. Adjusted with the nudge buttons; persisted in the backend per pane.
    const offsetB = ref(0);

    const isWindows = navigator.userAgent.includes('Windows');

    const playing = ref(false);
    // True from clicking play until the engine reports its first frame (first
    // launch builds the map cache and is slow) - drives the loading spinner.
    const booting = ref(false);
    const playError = ref<string | null>(null);

    // Playhead state (ms), fed by demo-player-status events.
    const posMs = ref(0);
    const lenMs = ref(0);
    const paused = ref(false);
    // The demo ran to its end and is frozen on the last frame. Not a real pause
    // (timescale is untouched), but we present it as paused and let Play replay.
    const atEnd = ref(false);
    const speed = ref(1);

    // Scrub display in seconds.
    const posSec = ref(0);
    const lenSec = ref(0);

    // Fine scrub: hold the scrub handle still for a moment and the bar "zooms"
    // to a narrow window around that point with 1 ms resolution, so you can land
    // on an exact millisecond. The whole bar then spans FINE_WINDOW_MS instead
    // of the full demo; releasing returns to the normal seconds bar.
    const FINE_WINDOW_MS = 3000; // total width of the zoomed window
    const FINE_DWELL_MS = 380; // hold-still time before zooming in
    const fineMode = ref(false);
    const fineMin = ref(0); // window bounds (ms)
    const fineMax = ref(0);
    let scrubPressed = false; // pointer is down on the scrub handle
    let dwellTimer: number | null = null;

    // Interaction guards (mirrors the validated test harness behaviour).
    let dragging = false;
    let seekHoldUntil = 0;
    let measured = false;
    let measureAttemptAt = 0;
    let seekTarget = 0;
    let lastArrowAt = -10000;
    let lastScrubSeekAt = 0;

    const embedRegion = ref<HTMLDivElement | null>(null);
    let unlisten: UnlistenFn | null = null;
    let unlistenClosed: UnlistenFn | null = null;
    let unlistenKey: UnlistenFn | null = null;
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
        atEnd.value = false;
        speed.value = 1;
        measured = false;
        measureAttemptAt = 0;
        seekTarget = 0;
        seekHoldUntil = 0;
        // fine scrub
        fineMode.value = false;
        scrubPressed = false;
        clearDwell();
        // pane 1 (comparison)
        pos1Ms.value = 0;
        len1Ms.value = 0;
        pos1Sec.value = 0;
        len1Sec.value = 0;
        measured1 = false;
        offsetB.value = 0;
    };

    // Seek the playhead to `ms`. In comparison mode this goes through the backend
    // so BOTH engines move together (each applying its sync offset); otherwise
    // it's a plain absolute seek on the single engine.
    const seekTo = (ms: number) => {
        const t = Math.round(ms);
        if (isCompare.value) tauri.demoPlayerSeekRelative(t).catch(() => {});
        else cmd(`seekdemo ${t}`);
    };

    const start = async (target: PlayTarget) => {
        if (!isWindows) {
            playError.value = 'The embedded demo player is only available on Windows.';
            return;
        }
        playError.value = null;
        resetPlayhead();
        // Show the loading spinner from the click until the engine reports its
        // first frame. The first oDFe launch is slow (it builds the map cache),
        // so without this the user stares at a black area wondering if it hung.
        booting.value = true;
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
            booting.value = false;
        }
    };

    // Start a side-by-side comparison of two demos (two engines, lockstep).
    const startCompare = async (c: CompareTarget) => {
        if (!isWindows) {
            playError.value = 'The embedded demo player is only available on Windows.';
            return;
        }
        playError.value = null;
        resetPlayhead();
        booting.value = true;
        try {
            const dpr = window.devicePixelRatio || 1;
            const dw = Math.round(window.screen.width * dpr);
            const dh = Math.round(window.screen.height * dpr);
            const rt = await tauri.engineDemoResolution(dw, dh).catch(() => null);
            lastAspect = rt && rt.aspect > 0 ? rt.aspect : 16 / 9;

            const region = computeRegion();
            if (!region) throw new Error('Render area not ready');
            await tauri.demoPlayerCompareStart(c.a.path, c.b.path, region, lastAspect);
            playing.value = true;
        } catch (e: any) {
            playError.value = e?.toString?.() ?? 'Failed to start comparison';
            playing.value = false;
            booting.value = false;
        }
    };

    // Start whichever mode the props request.
    const startActive = () => {
        if (props.compare) startCompare(props.compare);
        else if (props.demo) start(props.demo);
    };

    const stop = async () => {
        booting.value = false;
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
    //
    // Play/Pause control the play STATE; the speed buttons control the RATE
    // (timescale) independently. So "1x" is just another speed, separate from
    // the Play button.

    const doPause = () => {
        cmd('demopause 1');
        paused.value = true;
    };
    // Resume playback at the current speed. If the demo is frozen at its end,
    // seek back to the start first so Play replays it (offset-aware in compare).
    const play = () => {
        if (atEnd.value) seekTo(0);
        cmd(`demopause 0; timescale ${speed.value || 1}`);
        paused.value = false;
        atEnd.value = false;
    };
    // Set the playback RATE. Doesn't change play/pause state - if we're playing
    // it changes live; if paused it's the rate the next Play uses.
    const setSpeed = (x: number) => {
        cmd(`timescale ${x}`);
        speed.value = x;
    };
    // Spacebar / external toggle: Play when paused or ended, else Pause.
    const togglePause = () => {
        if (paused.value || atEnd.value) play();
        else doPause();
    };

    // ---- fine (millisecond) scrub ------------------------------------------

    // Slider binds in seconds normally, in milliseconds while zoomed.
    const sliderMin = computed(() => (fineMode.value ? fineMin.value : 0));
    const sliderMax = computed(() => (fineMode.value ? fineMax.value : scrubMax.value));
    const sliderStep = computed(() => (fineMode.value ? 1 : 1)); // 1 ms vs 1 s
    const sliderValue = computed(() => (fineMode.value ? Math.round(posMs.value) : posSec.value));

    const clearDwell = () => {
        if (dwellTimer !== null) {
            window.clearTimeout(dwellTimer);
            dwellTimer = null;
        }
    };
    // (Re)start the hold-still timer: if the handle doesn't move for FINE_DWELL_MS
    // while pressed, zoom in around the current position.
    const armDwell = () => {
        clearDwell();
        if (fineMode.value || !scrubPressed) return;
        dwellTimer = window.setTimeout(() => {
            if (scrubPressed && !fineMode.value) enterFineMode();
        }, FINE_DWELL_MS);
    };
    // Zoom in around the current playhead. The window is placed so the thumb
    // keeps the same on-screen position (no jump): its fraction of the bar is
    // preserved, so the pixel under the cursor stays put and small drags now
    // move milliseconds.
    const enterFineMode = () => {
        const center = Math.round(posMs.value);
        const span = Math.max(lenMs.value, 0);
        const frac = span > 0 ? Math.min(Math.max(center / span, 0), 1) : 0.5;
        let lo = Math.round(center - frac * FINE_WINDOW_MS);
        if (lo < 0) lo = 0;
        let hi = lo + FINE_WINDOW_MS;
        if (span > 0 && hi > span) {
            hi = span;
            lo = Math.max(0, hi - FINE_WINDOW_MS);
        }
        fineMin.value = lo;
        fineMax.value = hi;
        fineMode.value = true;
    };
    const exitFineMode = () => {
        fineMode.value = false;
        clearDwell();
    };
    // Keep the zoom usable when the user drags to an edge: slide the window so
    // they can keep going past it without releasing.
    const maybeShiftWindow = (ms: number) => {
        const edge = 50; // ms from the window edge that triggers a shift
        const span = lenMs.value;
        if (ms <= fineMin.value + edge && fineMin.value > 0) {
            const lo = Math.max(0, fineMin.value - FINE_WINDOW_MS / 2);
            fineMin.value = lo;
            fineMax.value = lo + FINE_WINDOW_MS;
        } else if (ms >= fineMax.value - edge && (span === 0 || fineMax.value < span)) {
            let hi = fineMax.value + FINE_WINDOW_MS / 2;
            if (span > 0 && hi > span) hi = span;
            fineMax.value = hi;
            fineMin.value = Math.max(0, hi - FINE_WINDOW_MS);
        }
    };

    const onScrubPointerDown = () => {
        scrubPressed = true;
        armDwell();
    };
    const onScrubPointerUp = () => {
        if (!scrubPressed) return;
        scrubPressed = false;
        clearDwell();
        // On release, lock in the exact position and drop back to the full bar.
        if (fineMode.value) {
            seekTo(posMs.value);
            seekHoldUntil = now() + 700;
            exitFineMode();
        }
    };

    const onScrubInput = (e: Event) => {
        dragging = true;
        const v = Number((e.target as HTMLInputElement).value);
        if (fineMode.value) {
            // value is milliseconds within the zoom window
            const ms = v;
            posMs.value = ms;
            posSec.value = ms / 1000;
            maybeShiftWindow(ms);
            const t = now();
            if (t - lastScrubSeekAt >= 60) {
                lastScrubSeekAt = t;
                seekTo(ms);
            }
            return;
        }
        // Coarse (seconds). Movement resets the hold-still timer, so the zoom
        // only triggers when the user actually stops on a spot.
        posSec.value = v;
        posMs.value = v * 1000;
        armDwell();
        // Live preview: seek the engine as the user drags so the picture
        // follows the handle, not just on release. Throttled so a fast drag
        // doesn't flood the control channel with seeks the engine can't keep
        // up with; the final exact seek still lands in onScrubChange.
        const t = now();
        if (t - lastScrubSeekAt >= 90) {
            lastScrubSeekAt = t;
            seekTo(v * 1000);
        }
    };
    const onScrubChange = (e: Event) => {
        const v = Number((e.target as HTMLInputElement).value);
        if (fineMode.value) {
            posMs.value = v;
            posSec.value = v / 1000;
            seekTo(v);
        } else {
            posSec.value = v;
            posMs.value = v * 1000;
            seekTo(v * 1000);
        }
        dragging = false;
        lastScrubSeekAt = now();
        seekHoldUntil = now() + 700;
    };

    // Jump the playhead by a fixed amount (ms). Used by the Up/Down 10 s seek.
    const seekBy = (deltaMs: number) => {
        const base = now() < seekHoldUntil ? seekTarget : posMs.value;
        seekTarget = base + deltaMs;
        if (seekTarget < 0) seekTarget = 0;
        if (lenMs.value > 0 && seekTarget > lenMs.value) seekTarget = lenMs.value;
        seekTo(seekTarget);
        const sec = Math.round(seekTarget / 1000);
        posSec.value = Math.min(Math.max(sec, 0), lenSec.value || sec);
        seekHoldUntil = now() + 300;
    };

    // Left / Right seek: 5 s nudge, accelerating to a 2 s scrub when held.
    const arrowSeek = (dir: number) => {
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
        seekTo(seekTarget);
        const sec = Math.round(seekTarget / 1000);
        posSec.value = Math.min(Math.max(sec, 0), lenSec.value || sec);
        seekHoldUntil = t + 300;
    };

    // ---- comparison sync offset --------------------------------------------

    // Nudge offsets are MULTIPLES OF 8 ms: Quake's simulation runs at 125 fps
    // (8 ms per frame), so 8 ms is one frame - the smallest meaningful step -
    // and the presets are 1 / 5 / 10 / 100 frames.
    const NUDGE_STEPS = [8, 40, 80, 800];

    // Nudge ONLY demo B by `deltaMs` so the two runs line up. Clicking again
    // keeps accumulating (it adds to the stored offset and never resets), and it
    // re-seeks pane 1 alone, so demo A doesn't jump back on every click.
    // Positive = demo B shifts later relative to A.
    const nudgeB = (deltaMs: number) => {
        if (!isCompare.value) return;
        offsetB.value += deltaMs;
        tauri.demoPlayerSetOffset(1, offsetB.value).catch(() => {});
        tauri.demoPlayerSeekPane(1, Math.round(posMs.value)).catch(() => {});
        seekHoldUntil = now() + 300;
    };
    const resetOffsetB = () => {
        if (!isCompare.value) return;
        offsetB.value = 0;
        tauri.demoPlayerSetOffset(1, 0).catch(() => {});
        tauri.demoPlayerSeekPane(1, Math.round(posMs.value)).catch(() => {});
        seekHoldUntil = now() + 300;
    };

    // Run a transport shortcut by normalized name. Shared by real keydowns
    // (when the launcher UI has focus) and by keys the engine forwards over the
    // control channel (when the demo render window has focus instead).
    const runShortcut = (name: string) => {
        if (!playing.value) return;
        switch (name) {
            case 'esc':   close(); break;
            case 'space': togglePause(); break;
            case 'up':    seekBy(10000); break;
            case 'down':  seekBy(-10000); break;
            case 'left':  arrowSeek(-1); break;
            case 'right': arrowSeek(1); break;
        }
    };

    const onKeydown = (e: KeyboardEvent) => {
        if (!playing.value) return;

        // Don't hijack keys while the user is typing in a text field (e.g. the
        // demo filter box below the player) - Space/arrows belong to the input.
        const el = e.target as HTMLElement | null;
        const tag = el?.tagName;
        if (tag === 'TEXTAREA' || (tag === 'INPUT' && (el as HTMLInputElement).type !== 'range')) {
            return;
        }

        let name = '';
        switch (e.key) {
            case 'Escape':     name = 'esc'; break;
            case ' ':
            case 'Spacebar':   name = 'space'; break;
            case 'ArrowUp':    name = 'up'; break;
            case 'ArrowDown':  name = 'down'; break;
            case 'ArrowLeft':  name = 'left'; break;
            case 'ArrowRight': name = 'right'; break;
            default: return;
        }
        e.preventDefault();
        runShortcut(name);
    };

    // ---- status events -----------------------------------------------------

    const onStatus = (s: DemoPlayerStatus) => {
        // Only the panel that owns the running session reacts - otherwise an
        // idle panel (e.g. the Player tab while the Demos overlay plays) would
        // also run the measurement seeks against the same engine.
        if (!playing.value) return;
        // First status from any engine = it's up and rendering; drop the spinner.
        booting.value = false;

        // Pane 1 (comparison right) only feeds its own timer + length.
        if (s.pane === 1) {
            pos1Ms.value = Math.max(0, s.time - s.start);
            len1Ms.value = s.total > s.start ? s.total - s.start : 0;
            if (!measured1 && len1Ms.value > 0) {
                len1Sec.value = Math.max(1, Math.round(len1Ms.value / 1000));
                measured1 = true;
            }
            if (len1Ms.value > 0) {
                const maxSec = Math.round(len1Ms.value / 1000);
                if (maxSec > 0) len1Sec.value = maxSec;
            }
            if (!(dragging || now() < seekHoldUntil)) {
                pos1Sec.value = Math.min(Math.max(Math.round(pos1Ms.value / 1000), 0), len1Sec.value || 0);
            }
            return;
        }

        // Pane 0 = primary (drives play/pause state + the shared scrub bar).
        posMs.value = Math.max(0, s.time - s.start);
        lenMs.value = s.total > s.start ? s.total - s.start : 0;
        paused.value = s.paused;

        if (!measured) {
            // Length is measured by seeking to a huge time (which transiently
            // hits the end), then back to 0 - so ignore `atend` until measured,
            // or the Pause indicator would flash on at startup. In comparison
            // mode the same seek measures BOTH engines (it's broadcast), so wait
            // until pane 1's length is known too before settling back to 0.
            const haveOther = !isCompare.value || len1Ms.value > 0;
            if (lenMs.value > 0 && haveOther) {
                lenSec.value = Math.max(1, Math.round(lenMs.value / 1000));
                if (len1Ms.value > 0) len1Sec.value = Math.max(1, Math.round(len1Ms.value / 1000));
                seekTo(0);
                measured = true;
                seekHoldUntil = now() + 500;
            } else if (now() - measureAttemptAt > 1200) {
                measureAttemptAt = now();
                seekTo(86400000);
            }
            return;
        }

        atEnd.value = s.atend;

        if (lenMs.value > 0) {
            const maxSec = Math.round(lenMs.value / 1000);
            if (maxSec > 0) lenSec.value = maxSec;
        }
        if (dragging || now() < seekHoldUntil) return;
        posSec.value = Math.min(Math.max(Math.round(posMs.value / 1000), 0), lenSec.value || 0);
    };

    // Scrub bar spans the longer of the two runs in comparison mode.
    const scrubMax = computed(() => Math.max(lenSec.value, isCompare.value ? len1Sec.value : 0) || 1);

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
    // Millisecond-precise readout for fine scrub: m:ss.mmm
    const fmtMs = (ms: number) => {
        const total = Math.max(0, Math.round(ms));
        const m = Math.floor(total / 60000);
        const s = Math.floor((total % 60000) / 1000);
        const mmm = total % 1000;
        return `${m}:${s.toString().padStart(2, '0')}.${mmm.toString().padStart(3, '0')}`;
    };

    // ---- lifecycle ---------------------------------------------------------

    // Restart when the target demo OR the comparison pair changes; stop when
    // both are cleared. Keyed on identity (paths) so an unrelated re-render
    // doesn't relaunch the engines.
    watch(
        () => [props.demo?.path ?? null, props.compare?.a.path ?? null, props.compare?.b.path ?? null].join('|'),
        () => {
            if (props.compare) startCompare(props.compare);
            else if (props.demo) start(props.demo);
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
        // Keys the engine forwarded because its render window had focus instead
        // of the launcher UI - run the same shortcut handler.
        unlistenKey = await listen<string>('demo-player-key', (e) => runShortcut(e.payload));
        unlistenMoved = await getCurrentWindow().onMoved(onWindowMoved);
        window.addEventListener('keydown', onKeydown);
        // Pointer can release anywhere, not just over the slider.
        window.addEventListener('pointerup', onScrubPointerUp);
        window.addEventListener('pointercancel', onScrubPointerUp);
        if (embedRegion.value) {
            resizeObs = new ResizeObserver(onRegionResize);
            resizeObs.observe(embedRegion.value);
        }
        startActive();
    });

    // keep-alive: stop the engine when the host view is backgrounded, resume
    // when it returns (the parent keeps the same demo/compare props).
    onActivated(() => {
        if ((props.demo || props.compare) && !playing.value) startActive();
    });
    onDeactivated(() => {
        stop();
    });

    onUnmounted(() => {
        stop();
        if (unlisten) unlisten();
        if (unlistenClosed) unlistenClosed();
        if (unlistenKey) unlistenKey();
        if (unlistenMoved) unlistenMoved();
        window.removeEventListener('keydown', onKeydown);
        window.removeEventListener('pointerup', onScrubPointerUp);
        window.removeEventListener('pointercancel', onScrubPointerUp);
        clearDwell();
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
            <span v-if="compare" class="flex-1 flex items-center justify-center gap-2 text-sm font-semibold truncate">
                <span class="truncate text-sky-300" :title="compare.a.name">{{ formatDemoName(compare.a.name) }}</span>
                <span class="text-neutral-500 flex-shrink-0">vs</span>
                <span class="truncate text-amber-300" :title="compare.b.name">{{ formatDemoName(compare.b.name) }}</span>
            </span>
            <span v-else class="flex-1 text-center text-sm font-semibold text-neutral-100 truncate">
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
            <!-- Booting: engine launched, waiting for its first frame. The first
                 launch builds the map cache and is slow, so show a spinner. -->
            <div
                v-if="booting"
                class="absolute inset-0 flex flex-col items-center justify-center text-neutral-300 pointer-events-none"
            >
                <div class="dr-spinner mb-4"></div>
                <div class="text-sm font-semibold">Loading demo…</div>
                <div class="text-xs text-neutral-500 mt-1">First launch builds the map cache - this can take a few seconds.</div>
            </div>
            <!-- Idle prompt (no demo playing, not booting). -->
            <div
                v-else-if="!playing"
                class="absolute inset-0 flex flex-col items-center justify-center text-neutral-500 pointer-events-none"
            >
                <div class="text-5xl mb-3">▶</div>
                <div class="text-sm">{{ isWindows ? 'Pick a demo to play' : 'The demo player is only available on Windows.' }}</div>
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
                    :class="(!paused && !atEnd) ? 'bg-emerald-500/30 text-emerald-200' : 'bg-white/5 hover:bg-white/10 text-neutral-200'"
                    title="Play / resume"
                    @click="play"
                >▶ Play</button>
                <button
                    class="px-3 py-1.5 rounded text-sm font-semibold"
                    :class="(paused || atEnd) ? 'bg-emerald-500/30 text-emerald-200' : 'bg-white/5 hover:bg-white/10 text-neutral-200'"
                    title="Pause"
                    @click="doPause"
                >⏸ Pause</button>
                <span class="w-px h-5 bg-white/10 mx-0.5"></span>
                <button
                    v-for="x in SPEEDS"
                    :key="x"
                    class="px-2.5 py-1.5 rounded text-sm font-semibold"
                    :class="speed === x ? 'bg-brand-500/30 text-brand-200' : 'bg-white/5 hover:bg-white/10 text-neutral-300'"
                    @click="setSpeed(x)"
                >{{ x + 'x' }}</button>

                <div class="flex-1 mx-2 relative">
                    <input
                        type="range"
                        class="w-full accent-brand-500"
                        :class="{ 'accent-amber-400': fineMode }"
                        :min="sliderMin"
                        :max="sliderMax"
                        :step="sliderStep"
                        :value="sliderValue"
                        @input="onScrubInput"
                        @change="onScrubChange"
                        @pointerdown="onScrubPointerDown"
                    />
                    <!-- Zoomed badge: shows you're in millisecond mode + the window. -->
                    <div
                        v-if="fineMode"
                        class="absolute -top-5 left-1/2 -translate-x-1/2 px-1.5 py-0.5 rounded bg-amber-500/20 text-amber-200 text-[10px] font-mono whitespace-nowrap pointer-events-none"
                    >ms zoom · {{ fmt(Math.floor(sliderMin/1000)) }}–{{ fmt(Math.ceil(sliderMax/1000)) }}</div>
                </div>
                <!-- Comparison: two timers (A / B) so you can read both runs. -->
                <span v-if="compare" class="font-mono text-sm tabular-nums text-right leading-tight">
                    <span class="text-sky-300">{{ fineMode ? fmtMs(posMs) : fmt(posSec) }}/{{ fmt(lenSec) }}</span>
                    <span class="text-neutral-600 mx-1">·</span>
                    <span class="text-amber-300">{{ fmt(pos1Sec) }}/{{ fmt(len1Sec) }}</span>
                </span>
                <span v-else class="font-mono text-sm tabular-nums w-28 text-right" :class="{ 'text-amber-300': fineMode }">
                    {{ fineMode ? fmtMs(posMs) : fmt(posSec) }} / {{ fmt(lenSec) }}
                </span>
            </div>

            <!-- Comparison: nudge demo B against A to line up the runs. The engine
                 only knows each demo's file start, so this is the manual sync. -->
            <div v-if="compare" class="mt-1.5 flex items-center gap-1.5 text-[11px] text-neutral-400 flex-wrap">
                <span class="text-amber-300 font-semibold mr-0.5">Sync demo B:</span>
                <button v-for="s in NUDGE_STEPS" :key="'m'+s" class="nudge" @click="nudgeB(-s)">-{{ s }}</button>
                <span class="text-neutral-600 mx-0.5">|</span>
                <button v-for="s in NUDGE_STEPS" :key="'p'+s" class="nudge" @click="nudgeB(s)">+{{ s }}</button>
                <button class="nudge ml-1" @click="resetOffsetB">reset</button>
                <span class="font-mono tabular-nums ml-1" :class="offsetB ? 'text-amber-300' : 'text-neutral-500'">
                    offset {{ offsetB > 0 ? '+' : '' }}{{ offsetB }}ms
                    <span v-if="offsetB" class="text-neutral-500">({{ (offsetB / 8).toFixed(offsetB % 8 ? 1 : 0) }}f)</span>
                </span>
            </div>
            <!-- Keyboard legend: what the shortcuts do while a demo plays. -->
            <div class="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px] text-neutral-500">
                <span class="inline-flex items-center gap-1.5">
                    <kbd class="kbd">Space</kbd>
                    <span>pause / resume</span>
                </span>
                <span class="inline-flex items-center gap-1.5">
                    <kbd class="kbd">←</kbd><kbd class="kbd">→</kbd>
                    <span>seek 5 s <span class="text-neutral-600">(hold to scrub)</span></span>
                </span>
                <span class="inline-flex items-center gap-1.5">
                    <kbd class="kbd">↑</kbd><kbd class="kbd">↓</kbd>
                    <span>seek 10 s</span>
                </span>
                <span class="inline-flex items-center gap-1.5">
                    <kbd class="kbd">Esc</kbd>
                    <span>close player</span>
                </span>
                <span class="inline-flex items-center gap-1.5">
                    <span class="text-amber-400">⤢</span>
                    <span>hold the scrub still to zoom to milliseconds</span>
                </span>
            </div>
        </div>
    </div>
</template>

<style scoped>
    .dr-spinner {
        width: 2.5rem;
        height: 2.5rem;
        border-radius: 9999px;
        border: 3px solid rgb(255 255 255 / 0.12);
        border-top-color: var(--brand-500, #3b82f6);
        animation: dr-spin 0.8s linear infinite;
    }
    @keyframes dr-spin {
        to { transform: rotate(360deg); }
    }

    .nudge {
        padding: 0.1rem 0.45rem;
        border-radius: 0.25rem;
        background: rgb(255 255 255 / 0.06);
        color: rgb(212 212 212);
        font-size: 11px;
        line-height: 1.2;
    }
    .nudge:hover {
        background: rgb(255 255 255 / 0.12);
    }

    .kbd {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        min-width: 1.4rem;
        height: 1.25rem;
        padding: 0 0.3rem;
        border: 1px solid rgb(255 255 255 / 0.15);
        border-bottom-width: 2px;
        border-radius: 0.25rem;
        background: rgb(255 255 255 / 0.06);
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 10px;
        line-height: 1;
        color: rgb(212 212 212);
    }
</style>
