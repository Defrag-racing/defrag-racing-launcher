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
    import { LazyStore } from '@tauri-apps/plugin-store';
    import { tauri, type DemoPlayerStatus } from '../lib/tauri';

    /** The demo to play: absolute `path` on disk + a `name` for the banner. */
    export interface PlayTarget {
        path: string;
        name: string;
    }

    /** 2-4 demos to compare, tiled into a grid (premium feature). */
    export interface CompareTarget {
        demos: PlayTarget[];
    }

    const props = defineProps<{
        demo: PlayTarget | null;
        compare?: CompareTarget | null;
    }>();
    const emit = defineEmits<{ (e: 'close'): void }>();

    // Comparison mode: 2-4 engines tiled in a grid, driven in lockstep.
    const isCompare = computed(() => !!props.compare && props.compare.demos.length >= 2);
    const paneCount = computed(() => (props.compare ? props.compare.demos.length : 1));

    // Per-pane mirror of the playhead, for each demo's own timer. Index = pane.
    // Pane 0 also feeds the primary refs below (it drives the shared scrub).
    interface PaneTimer { posMs: number; lenMs: number; posSec: number; lenSec: number; measured: boolean; atEnd: boolean }
    const paneTimers = ref<PaneTimer[]>([]);
    // Sync offset (ms) per pane so runs with different lead-ins line up. Pane 0
    // is the anchor (offset 0); panes 1+ are nudged against it.
    const offsets = ref<number[]>([]);

    // Per-pane audio: a master volume (0..1) plus a mute toggle. Quake's master
    // volume is s_volume; we send `s_volume <v>` per pane (0 when muted). The
    // single viewer has one slider+mute; in a comparison every demo gets its own
    // so you can balance them. By default only demo A is audible (the extra
    // demos start muted) so several runs don't pile into noise.
    //
    // 0.3 out of the box: a demo is something you glance at, usually with music
    // or a stream already playing, and 0.8 arrived like a slap. Whatever the
    // user sets instead is remembered - in a plugin-store file rather than
    // localStorage, which does not survive a restart in this webview (see the
    // same note in Dashboard.vue).
    const DEFAULT_VOL = 0.3;
    const prefsStore = new LazyStore('player-prefs.json');
    const VOL_FIELD = 'volume';
    const lastVol = ref(DEFAULT_VOL);
    /** Has the user moved the slider this session? Then a late load must not undo it. */
    let volTouched = false;
    let volSaveTimer: ReturnType<typeof setTimeout> | undefined;
    const volume = ref<number[]>([]);
    const muted = ref<boolean[]>([]);

    const loadVolume = async () => {
        try {
            const v = await prefsStore.get<number>(VOL_FIELD);
            if (typeof v !== 'number' || Number.isNaN(v)) return;
            lastVol.value = Math.min(1, Math.max(0, v));
            if (volTouched) return;
            volume.value = volume.value.map(() => lastVol.value);
            if (playing.value) applyAudio();
        } catch {
            // No stored preference, or the store is unreadable: keep the default.
        }
    };

    // Debounced: a slider drag fires a stream of input events and each write
    // touches the disk.
    const saveVolume = (v: number) => {
        lastVol.value = v;
        clearTimeout(volSaveTimer);
        volSaveTimer = setTimeout(() => {
            prefsStore
                .set(VOL_FIELD, v)
                .then(() => prefsStore.save())
                .catch(() => {});
        }, 400);
    };

    const blankTimer = (): PaneTimer => ({ posMs: 0, lenMs: 0, posSec: 0, lenSec: 0, measured: false, atEnd: false });
    const initPaneArrays = () => {
        const n = paneCount.value;
        paneTimers.value = Array.from({ length: n }, blankTimer);
        offsets.value = Array.from({ length: n }, () => 0);
        volume.value = Array.from({ length: n }, () => lastVol.value);
        // Default: demo A audible, the rest muted (avoids overlapping audio).
        muted.value = Array.from({ length: n }, (_, i) => isCompare.value && i > 0);
    };

    // The effective volume for a pane (0 while muted), clamped to Quake's 0..1.
    const effVol = (i: number) => (muted.value[i] ? 0 : Math.min(1, Math.max(0, volume.value[i] ?? lastVol.value)));
    // Push one pane's current volume to its engine.
    const applyVol = (i: number) => {
        tauri.demoPlayerPaneCommand(i, `s_volume ${effVol(i)}`).catch(() => {});
    };
    // Push every pane's audio state to its engine (after start).
    const applyAudio = () => {
        muted.value.forEach((_, i) => applyVol(i));
    };
    const toggleMute = (i: number) => {
        if (i < 0 || i >= muted.value.length) return;
        muted.value[i] = !muted.value[i];
        applyVol(i);
    };
    // Slider moved: set the volume and, since the user is asking for sound,
    // auto-unmute (dragging a muted slider up should be audible).
    const onVolInput = (i: number, e: Event) => {
        if (i < 0 || i >= volume.value.length) return;
        const v = parseFloat((e.target as HTMLInputElement).value);
        if (Number.isNaN(v)) return;
        volume.value[i] = v;
        if (v > 0) muted.value[i] = false;
        applyVol(i);
        // Remembered from pane 0 only: in a comparison the other sliders are
        // there to balance THIS pair of runs, not to set a preference.
        if (i === 0) {
            volTouched = true;
            saveVolume(v);
        }
    };

    // The embedded player runs on Windows and Linux (X11 / XWayland). On Linux,
    // if the session is native Wayland the backend returns a clear "use X11"
    // message; we still let the attempt through so that message can surface.
    const isEmbedSupported =
        navigator.userAgent.includes('Windows') || navigator.userAgent.includes('Linux');

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
    let lastMoveX = 0; // pointer position when the dwell timer was last (re)armed
    let lastMoveY = 0;
    const MOVE_THRESHOLD = 3; // px of movement that counts as "still moving"

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

    const SPEEDS = [0.1, 0.2, 0.3, 0.4, 0.5, 0.75, 1, 2, 4, 8];

    // Per-pane accent colour + letter (A/B/C/D) so each demo is identifiable in
    // the name bar, the timers, and its sync row.
    const PANE_COLORS = ['text-sky-300', 'text-amber-300', 'text-emerald-300', 'text-fuchsia-300'];
    const paneColor = (i: number) => PANE_COLORS[i % PANE_COLORS.length];
    const paneLetter = (i: number) => String.fromCharCode(65 + i);
    // Border accent colours (sky/amber/emerald/fuchsia 400), matching the names
    // and sync rows, drawn as a frame around each pane so you know which is which.
    const PANE_BORDERS = ['#38bdf8', '#fbbf24', '#34d399', '#e879f9'];
    // Grid layout mirroring the backend's pane_region: 2=2x1, 3=3x1, 4=2x2.
    const gridLayout = computed(() => {
        const n = paneCount.value;
        if (n <= 2) return { cols: n, rows: 1 };
        if (n === 3) return { cols: 3, rows: 1 };
        return { cols: 2, rows: 2 };
    });

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
        // comparison panes
        initPaneArrays();
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
        if (!isEmbedSupported) {
            playError.value = 'The embedded demo player is only available on Windows and Linux.';
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
            // Apply the single pane's volume/mute once the engine is up; the
            // control channel buffers it until the engine connects.
            applyAudio();
        } catch (e: any) {
            playError.value = e?.toString?.() ?? 'Failed to start playback';
            playing.value = false;
            booting.value = false;
        }
    };

    // Start a side-by-side comparison of two demos (two engines, lockstep).
    const startCompare = async (c: CompareTarget) => {
        if (!isEmbedSupported) {
            playError.value = 'The embedded demo player is only available on Windows and Linux.';
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
            await tauri.demoPlayerCompareStart(c.demos.map((d) => d.path), region, lastAspect);
            playing.value = true;
            // Apply the default audio layout (only A audible) once the engines
            // are up; the control channel buffers these until each connects.
            applyAudio();
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

    // The scrub bar works in MILLISECONDS the whole time (not seconds), so a
    // short demo gets hundreds of scrub positions instead of one per second -
    // dragging a 5 s run is now smooth, not 5 coarse jumps. The step is one
    // Quake frame (8 ms at 125 fps): finer than that shows the same frame, so 8
    // ms is the finest *meaningful* resolution. The on-screen pixel width still
    // limits how precisely you can land on a long demo - that's what hold-to-zoom
    // is for - but short/medium demos are now frame-accurate by drag alone.
    const FRAME_MS = 8;
    const sliderMin = computed(() => (fineMode.value ? fineMin.value : 0));
    const sliderMax = computed(() => (fineMode.value ? fineMax.value : (maxLenMs.value || scrubMax.value * 1000)));
    const sliderStep = computed(() => FRAME_MS);
    const sliderValue = computed(() => Math.round(posMs.value));

    // Filled fraction of the scrub bar (0-100), for the coloured fill + to know
    // where the thumb sits.
    const scrubPercent = computed(() => {
        const min = sliderMin.value;
        const max = sliderMax.value;
        if (max <= min) return 0;
        return Math.min(100, Math.max(0, ((sliderValue.value - min) / (max - min)) * 100));
    });
    // The bar turns amber while zoomed to milliseconds; blue otherwise. The fill
    // is a gradient so we control its colour (native accent can't change live).
    const scrubStyle = computed(() => {
        const fill = fineMode.value ? '#fbbf24' : '#3b82f6';
        const track = 'rgba(255,255,255,0.18)';
        const p = scrubPercent.value;
        return { background: `linear-gradient(90deg, ${fill} 0%, ${fill} ${p}%, ${track} ${p}%, ${track} 100%)` };
    });

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
        const span = Math.max(maxLenMs.value, 0);
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
    // While the user scrubs, pause the engine so its own advancing playback time
    // doesn't fight the stream of seeks we issue as the handle moves. Without
    // this, seeking a *playing* demo desyncs the playhead and (on Linux) can
    // freeze it outright. We remember whether it was playing and resume on
    // release. paused.value is deliberately NOT touched so the UI keeps showing
    // "playing" throughout (no flicker).
    let scrubWasPlaying = false;
    const resumeAfterScrub = () => {
        if (!scrubWasPlaying) return;
        scrubWasPlaying = false;
        cmd(`demopause 0; timescale ${speed.value || 1}`);
    };
    const onScrubPointerDown = (e: PointerEvent) => {
        scrubPressed = true;
        lastMoveX = e.clientX;
        lastMoveY = e.clientY;
        armDwell();
        if (!paused.value && !atEnd.value) {
            scrubWasPlaying = true;
            cmd('demopause 1');
        } else {
            scrubWasPlaying = false;
        }
    };
    // Real mouse movement (not just value changes) cancels/restarts the dwell:
    // with a 1 s step the slider fires no `input` while you drag within the same
    // second, so without this the zoom would wrongly trigger mid-move. Any
    // movement past a small threshold re-arms the timer, so the zoom only fires
    // when the pointer is genuinely held still.
    const onScrubPointerMove = (e: PointerEvent) => {
        if (!scrubPressed || fineMode.value) return;
        if (Math.abs(e.clientX - lastMoveX) + Math.abs(e.clientY - lastMoveY) >= MOVE_THRESHOLD) {
            lastMoveX = e.clientX;
            lastMoveY = e.clientY;
            armDwell();
        }
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
        resumeAfterScrub();
    };

    const onScrubInput = (e: Event) => {
        dragging = true;
        // The slider value is always milliseconds now. While zoomed it's clamped
        // to the fixed window bounds; otherwise it spans the whole demo.
        let ms = Number((e.target as HTMLInputElement).value);
        if (fineMode.value) ms = Math.min(Math.max(ms, fineMin.value), fineMax.value);
        posMs.value = ms;
        posSec.value = ms / 1000;
        // Live preview: seek as the user drags so the picture follows the handle.
        // Throttled so a fast drag doesn't flood the control channel (tighter
        // while zoomed, where small moves matter); the exact seek lands on release.
        const t = now();
        const throttle = fineMode.value ? 60 : 90;
        if (t - lastScrubSeekAt >= throttle) {
            lastScrubSeekAt = t;
            seekTo(ms);
        }
    };
    const onScrubChange = (e: Event) => {
        const v = Number((e.target as HTMLInputElement).value);
        {
            posMs.value = v;
            posSec.value = v / 1000;
            seekTo(v);
        }
        dragging = false;
        lastScrubSeekAt = now();
        seekHoldUntil = now() + 700;
        resumeAfterScrub();
    };

    // Jump the playhead by a fixed amount (ms). Used by the Up/Down 10 s seek.
    const seekBy = (deltaMs: number) => {
        const base = now() < seekHoldUntil ? seekTarget : posMs.value;
        seekTarget = base + deltaMs;
        if (seekTarget < 0) seekTarget = 0;
        if (maxLenMs.value > 0 && seekTarget > maxLenMs.value) seekTarget = maxLenMs.value;
        seekTo(seekTarget);
        const sec = Math.round(seekTarget / 1000);
        posSec.value = Math.min(Math.max(sec, 0), scrubMax.value || sec);
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
        if (maxLenMs.value > 0 && seekTarget > maxLenMs.value) seekTarget = maxLenMs.value;
        seekTo(seekTarget);
        const sec = Math.round(seekTarget / 1000);
        posSec.value = Math.min(Math.max(sec, 0), scrubMax.value || sec);
        seekHoldUntil = t + 300;
    };

    // ---- comparison sync offset --------------------------------------------

    // Nudge offsets are MULTIPLES OF 8 ms: Quake's simulation runs at 125 fps
    // (8 ms per frame), so 8 ms is one frame - the smallest meaningful step.
    // Presets: 1 / 5 / 10 / 100 / 1000 frames. Rendered as -8000..-8 | +8..+8000
    // so -8 sits right next to +8.
    const NUDGE_STEPS = [8, 40, 80, 800, 8000];
    const NUDGE_STEPS_DESC = [...NUDGE_STEPS].reverse();

    // Nudge ONLY pane `idx` by `deltaMs` so its run lines up with the anchor
    // (pane 0). Clicking again keeps accumulating (never resets), and it
    // re-seeks that pane alone, so the others don't jump back on every click.
    const nudgePane = (idx: number, deltaMs: number) => {
        if (!isCompare.value || idx <= 0 || idx >= offsets.value.length) return;
        offsets.value[idx] += deltaMs;
        tauri.demoPlayerSetOffset(idx, offsets.value[idx]).catch(() => {});
        tauri.demoPlayerSeekPane(idx, Math.round(posMs.value)).catch(() => {});
        seekHoldUntil = now() + 300;
    };
    const resetOffset = (idx: number) => {
        if (!isCompare.value || idx <= 0 || idx >= offsets.value.length) return;
        offsets.value[idx] = 0;
        tauri.demoPlayerSetOffset(idx, 0).catch(() => {});
        tauri.demoPlayerSeekPane(idx, Math.round(posMs.value)).catch(() => {});
        seekHoldUntil = now() + 300;
    };

    // Step exactly one Quake frame (8 ms) at a time, for frame-by-frame study.
    // Pauses first so you're stepping through a still image, and snaps the target
    // to the 8 ms frame grid so you always land on a real frame.
    const stepFrame = (frames: number) => {
        if (!paused.value) doPause();
        const base = now() < seekHoldUntil ? seekTarget : posMs.value;
        let target = Math.round((base + frames * FRAME_MS) / FRAME_MS) * FRAME_MS;
        if (target < 0) target = 0;
        if (maxLenMs.value > 0 && target > maxLenMs.value) target = maxLenMs.value;
        seekTarget = target;
        seekTo(target);
        posMs.value = target;
        posSec.value = Math.min(Math.max(Math.round(target / 1000), 0), scrubMax.value || 0);
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

        // Shift + Left/Right steps one frame at a time (only works with the
        // launcher focused; the engine forwards plain arrows without modifiers).
        if (e.shiftKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
            e.preventDefault();
            stepFrame(e.key === 'ArrowRight' ? 1 : -1);
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

        // Update this pane's own timer (used for its label + the measure check).
        const pt = paneTimers.value[s.pane];
        if (pt) {
            pt.posMs = Math.max(0, s.time - s.start);
            pt.lenMs = s.total > s.start ? s.total - s.start : 0;
            pt.atEnd = s.atend;
            if (pt.lenMs > 0) {
                pt.lenSec = Math.max(1, Math.round(pt.lenMs / 1000));
                pt.measured = true;
            }
            if (!(dragging || now() < seekHoldUntil)) {
                pt.posSec = Math.min(Math.max(Math.round(pt.posMs / 1000), 0), pt.lenSec || 0);
            }
        }

        // Only pane 0 drives measurement + the play/pause state.
        if (s.pane === 0) {
            lenMs.value = s.total > s.start ? s.total - s.start : 0;
            // Don't sync the paused state while we've paused the engine purely
            // for scrubbing - the UI should keep showing "playing" and flip back
            // only once we've actually resumed on release.
            if (!scrubWasPlaying) paused.value = s.paused;

            if (!measured) {
                // Length is measured by seeking to a huge time (which transiently
                // hits the end), then back to 0 - so ignore `atend` until
                // measured. In comparison mode the broadcast seek measures EVERY
                // engine, so wait until all panes' lengths are known.
                const allMeasured = !isCompare.value || paneTimers.value.every((p) => p.lenMs > 0);
                if (lenMs.value > 0 && allMeasured) {
                    lenSec.value = Math.max(1, Math.round(lenMs.value / 1000));
                    seekTo(0);
                    measured = true;
                    seekHoldUntil = now() + 500;
                } else if (now() - measureAttemptAt > 1200) {
                    measureAttemptAt = now();
                    seekTo(86400000);
                }
                return;
            }
            if (lenMs.value > 0) {
                const maxSec = Math.round(lenMs.value / 1000);
                if (maxSec > 0) lenSec.value = maxSec;
            }
        }

        if (!measured) return;

        // Shared playhead tracks the demo that's FURTHEST along, so when a
        // shorter demo freezes at its end the handle keeps moving with the
        // longer ones until they all finish. Each pane reports its own demo time
        // (which includes its sync offset), so subtract the offset to recover
        // the common playhead. "Ended" is only true once EVERY demo has ended.
        if (!(dragging || now() < seekHoldUntil)) {
            let head = 0;
            let allEnded = true;
            paneTimers.value.forEach((p, i) => {
                head = Math.max(head, p.posMs - (offsets.value[i] ?? 0));
                if (!p.atEnd) allEnded = false;
            });
            posMs.value = Math.max(0, head);
            posSec.value = Math.min(Math.max(Math.round(posMs.value / 1000), 0), scrubMax.value);
            atEnd.value = isCompare.value ? allEnded : (paneTimers.value[0]?.atEnd ?? false);
        }
    };

    // Longest run across all compared demos, in seconds (scrub max) and ms.
    const scrubMax = computed(() => {
        let m = lenSec.value;
        if (isCompare.value) for (const p of paneTimers.value) m = Math.max(m, p.lenSec);
        return m || 1;
    });
    const maxLenMs = computed(() => {
        let m = lenMs.value;
        if (isCompare.value) for (const p of paneTimers.value) m = Math.max(m, p.lenMs);
        return m;
    });

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
        () => [props.demo?.path ?? null, ...(props.compare?.demos.map((d) => d.path) ?? [])].join('|'),
        () => {
            if (props.compare) startCompare(props.compare);
            else if (props.demo) start(props.demo);
            else stop();
        },
    );

    onMounted(async () => {
        // Before startActive() below, so the first demo of the session already
        // opens at the remembered level; loadVolume re-applies if it loses the
        // race anyway.
        void loadVolume();
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
        // Pointer can release/move anywhere, not just over the slider.
        window.addEventListener('pointerup', onScrubPointerUp);
        window.addEventListener('pointercancel', onScrubPointerUp);
        window.addEventListener('pointermove', onScrubPointerMove);
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
        window.removeEventListener('pointermove', onScrubPointerMove);
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
                <template v-for="(d, i) in compare.demos" :key="i">
                    <span v-if="i > 0" class="text-neutral-500 flex-shrink-0">vs</span>
                    <span class="flex items-center gap-1 truncate min-w-0">
                        <button
                            class="flex-shrink-0 px-1 rounded hover:bg-white/10"
                            :class="(muted[i] ?? false) ? 'opacity-50' : ''"
                            :title="(muted[i] ?? false) ? `Unmute demo ${paneLetter(i)}` : `Mute demo ${paneLetter(i)}`"
                            @click="toggleMute(i)"
                        >{{ (muted[i] ?? false) ? '🔇' : '🔊' }}</button>
                        <input
                            type="range" min="0" max="1" step="0.05"
                            class="vol vol-sm flex-shrink-0"
                            :value="effVol(i)"
                            :title="`Volume - demo ${paneLetter(i)}`"
                            @input="onVolInput(i, $event)"
                        />
                        <span class="truncate" :class="paneColor(i)" :title="d.name">
                            <span class="opacity-60">{{ paneLetter(i) }}</span> {{ formatDemoName(d.name) }}
                        </span>
                    </span>
                </template>
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
            <!-- Comparison: a coloured frame per pane so you can tell which demo
                 is which. The engines are inset inside their cells (backend), so
                 these borders sit in the margin and stay visible. -->
            <div
                v-if="isCompare && compare"
                class="absolute inset-0 pointer-events-none"
                :style="{
                    display: 'grid',
                    gridTemplateColumns: `repeat(${gridLayout.cols}, 1fr)`,
                    gridTemplateRows: `repeat(${gridLayout.rows}, 1fr)`,
                    gap: '6px',
                }"
            >
                <div
                    v-for="n in paneCount"
                    :key="n"
                    class="rounded-sm"
                    :style="{ border: `2px solid ${PANE_BORDERS[(n - 1) % PANE_BORDERS.length]}`, boxSizing: 'border-box' }"
                ></div>
            </div>
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
                <div class="text-sm">{{ isEmbedSupported ? 'Pick a demo to play' : 'The demo player is only available on Windows and Linux.' }}</div>
            </div>
        </div>

        <div
            v-if="playError"
            class="flex-shrink-0 px-3 py-2 bg-red-500/15 border-t border-red-500/30 text-red-300 text-xs"
        >{{ playError }}</div>

        <!-- Transport bar -->
        <div class="flex-shrink-0 border-t border-white/10 bg-neutral-900 px-3 py-2">
            <!-- Speeds (timescale) on their own row: there are a lot of them and
                 they ate most of the width, leaving the scrub bar cramped. Giving
                 them a dedicated row lets the scrub bar below span the full width. -->
            <div class="flex items-center justify-center gap-1.5 flex-wrap mb-1.5">
                <span class="text-[11px] uppercase tracking-wide text-neutral-500 mr-1">Speed</span>
                <template v-for="x in SPEEDS" :key="x">
                    <!-- small gap between the fractional speeds and the whole-number ones -->
                    <span v-if="x === 1" class="w-2 flex-shrink-0" aria-hidden="true"></span>
                    <button
                        class="px-2.5 py-1 rounded text-sm font-semibold"
                        :class="speed === x ? 'bg-brand-500/30 text-brand-200' : 'bg-white/5 hover:bg-white/10 text-neutral-300'"
                        @click="setSpeed(x)"
                    >{{ x + 'x' }}</button>
                </template>
            </div>

            <!-- Playback + scrub -->
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
                <!-- Single viewer audio: mute toggle + volume slider (comparison
                     panes get their own controls in the header instead). -->
                <span v-if="!compare" class="flex items-center gap-1 flex-shrink-0">
                    <button
                        class="px-1.5 py-1 rounded text-sm bg-white/5 hover:bg-white/10"
                        :class="(muted[0] ?? false) ? 'opacity-50' : ''"
                        :title="(muted[0] ?? false) ? 'Unmute' : 'Mute'"
                        @click="toggleMute(0)"
                    >{{ (muted[0] ?? false) ? '🔇' : '🔊' }}</button>
                    <input
                        type="range" min="0" max="1" step="0.05"
                        class="vol"
                        :value="effVol(0)"
                        title="Volume"
                        @input="onVolInput(0, $event)"
                    />
                </span>
                <span class="w-px h-5 bg-white/10 mx-0.5"></span>
                <input
                    type="range"
                    class="scrub flex-1 mx-2"
                    :class="{ 'scrub-fine': fineMode }"
                    :style="scrubStyle"
                    :min="sliderMin"
                    :max="sliderMax"
                    :step="sliderStep"
                    :value="sliderValue"
                    @input="onScrubInput"
                    @change="onScrubChange"
                    @pointerdown="onScrubPointerDown"
                />
                <!-- Comparison: one timer per demo so you can read every run.
                     nowrap so switching to ms precision in zoom mode can't wrap
                     the timer onto a second line (that would grow the transport
                     bar and trigger an engine-window resize). -->
                <span v-if="compare" class="font-mono text-xs tabular-nums text-right leading-tight whitespace-nowrap flex-shrink-0">
                    <template v-for="(p, i) in paneTimers" :key="i">
                        <span v-if="i > 0" class="text-neutral-600 mx-1">·</span>
                        <span :class="paneColor(i)">{{ (i === 0 && fineMode) ? fmtMs(posMs) : fmt(p.posSec) }}/{{ fmt(p.lenSec) }}</span>
                    </template>
                </span>
                <span v-else class="font-mono text-sm tabular-nums w-36 text-right whitespace-nowrap flex-shrink-0" :class="{ 'text-amber-300': fineMode }">
                    {{ fineMode ? fmtMs(posMs) : fmt(posSec) }} / {{ fmt(lenSec) }}
                </span>
            </div>

            <!-- ms-zoom readout on its own fixed-height line, so toggling zoom
                 on/off never changes the transport bar height. Previously the
                 indicator + the longer ms timer grew the bar, which shrank the
                 demo area and forced the engine to resize/refresh its window. -->
            <div class="h-4 mt-0.5 flex items-center justify-center">
                <span
                    v-if="fineMode"
                    class="px-1.5 rounded bg-amber-500/25 text-amber-200 text-[10px] font-mono whitespace-nowrap leading-none"
                >ms zoom {{ fmt(Math.floor(sliderMin/1000)) }}–{{ fmt(Math.ceil(sliderMax/1000)) }}</span>
            </div>

            <!-- Comparison: nudge each demo against demo A to line the runs up.
                 The engine only knows each demo's file start, so this is the
                 manual sync. One row per demo (A is the anchor). Steps are
                 multiples of 8 ms = whole frames; -8 sits next to +8. -->
            <div v-if="compare && offsets.length" class="mt-1.5 space-y-1">
                <div
                    v-for="i in (paneCount - 1)"
                    :key="i"
                    class="flex items-center gap-1.5 text-[11px] text-neutral-400 flex-wrap"
                >
                    <span class="font-semibold mr-0.5 w-12 flex-shrink-0" :class="paneColor(i)">Sync {{ paneLetter(i) }}:</span>
                    <button v-for="s in NUDGE_STEPS_DESC" :key="'m'+s" class="nudge" @click="nudgePane(i, -s)">-{{ s }}</button>
                    <span class="text-neutral-600 mx-0.5">|</span>
                    <button v-for="s in NUDGE_STEPS" :key="'p'+s" class="nudge" @click="nudgePane(i, s)">+{{ s }}</button>
                    <span class="font-mono tabular-nums ml-1.5 w-24" :class="(offsets[i] ?? 0) ? paneColor(i) : 'text-neutral-500'">
                        {{ (offsets[i] ?? 0) > 0 ? '+' : '' }}{{ offsets[i] ?? 0 }}ms
                        <span v-if="offsets[i]" class="text-neutral-500">({{ ((offsets[i] ?? 0) / 8).toFixed((offsets[i] ?? 0) % 8 ? 1 : 0) }}f)</span>
                    </span>
                    <button class="nudge-reset ml-1" @click="resetOffset(i)" title="Reset this demo's sync to 0">reset</button>
                </div>
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
                    <kbd class="kbd">Shift</kbd><kbd class="kbd">←</kbd><kbd class="kbd">→</kbd>
                    <span>step 1 frame</span>
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
    .nudge-reset {
        padding: 0.1rem 0.55rem;
        border-radius: 0.25rem;
        background: rgb(244 63 94 / 0.14); /* rose */
        color: rgb(253 164 175);
        font-size: 11px;
        line-height: 1.2;
    }
    .nudge-reset:hover {
        background: rgb(244 63 94 / 0.26);
    }

    /* Volume slider: a slim version of the scrub bar. Default width suits the
       single viewer; .vol-sm narrows it for the per-demo controls in the
       comparison header. */
    .vol {
        -webkit-appearance: none;
        appearance: none;
        width: 72px;
        height: 4px;
        border-radius: 9999px;
        background: rgb(255 255 255 / 0.18);
        cursor: pointer;
        outline: none;
    }
    .vol.vol-sm {
        width: 48px;
    }
    .vol::-webkit-slider-thumb {
        -webkit-appearance: none;
        appearance: none;
        width: 12px;
        height: 12px;
        border-radius: 9999px;
        background: rgb(229 229 229);
        cursor: pointer;
    }
    .vol::-moz-range-thumb {
        width: 12px;
        height: 12px;
        border: none;
        border-radius: 9999px;
        background: rgb(229 229 229);
        cursor: pointer;
    }

    /* Custom scrub bar: a rounded track (its fill colour is set inline so it can
       turn amber while zoomed) and a big, obvious thumb that grows further in
       millisecond-zoom mode. */
    .scrub {
        -webkit-appearance: none;
        appearance: none;
        height: 6px;
        border-radius: 9999px;
        cursor: pointer;
        outline: none;
    }
    .scrub::-webkit-slider-thumb {
        -webkit-appearance: none;
        appearance: none;
        width: 16px;
        height: 16px;
        border-radius: 9999px;
        background: #ffffff;
        border: 2px solid #3b82f6;
        box-shadow: 0 0 0 3px rgb(59 130 246 / 0.35), 0 1px 3px rgb(0 0 0 / 0.5);
        transition: width 0.1s ease, height 0.1s ease;
    }
    .scrub::-moz-range-thumb {
        width: 16px;
        height: 16px;
        border-radius: 9999px;
        background: #ffffff;
        border: 2px solid #3b82f6;
        box-shadow: 0 0 0 3px rgb(59 130 246 / 0.35);
    }
    .scrub-fine::-webkit-slider-thumb {
        width: 24px;
        height: 24px;
        border-color: #f59e0b;
        box-shadow: 0 0 0 4px rgb(245 158 11 / 0.4), 0 1px 4px rgb(0 0 0 / 0.6);
    }
    .scrub-fine::-moz-range-thumb {
        width: 24px;
        height: 24px;
        border-color: #f59e0b;
        box-shadow: 0 0 0 4px rgb(245 158 11 / 0.4);
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
