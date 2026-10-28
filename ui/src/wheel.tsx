import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import type { UnlistenFn } from '@tauri-apps/api/event';
import * as ipc from './ipc';
import { performAction } from './actions';
import { categoryLabel } from './catalog';
import { groupForKey, keyLabel, wheelKeyFromEventKey } from './keys';
import { glyphFor } from './icons';
import { indexForPoint, layoutRing, radialWedgePath, type RingGeometry, type SectorGeometry } from './geometry';
import type { Segment, Settings, WheelContents, WheelGroup } from './types';

const VIEW = 640;
const CX = 320;
const CY = 320;
/** Reference proportions (a 360-unit box): plate 151, petals 56→146, hub 49. */
const PLATE_R = 312;
const OUTER = 300;
/** The hub is a pill, not a disc, so the petals can start close to the centre —
 *  this is what makes them read as large rather than as a thin ring. */
const INNER = 138;
/** Angular gap between neighbours, and how much a selected wedge widens. */
const GAP_DEG = 0.7;
const SPREAD_DEG = 1.3;
/** A selected wedge grows inward, outward and sideways at once. */
const GROW_OUT = 7;
const GROW_IN = 2;
/** Corner radius of every wedge; the reference rounds both the inner and outer tips. */
const CORNER = 20;
/** Copy of the selected wedge, dropped below it to lift it off the ring. */
const COPY_DROP = 6;

const STAGE_FRACTION = 0.82;

const EMPTY: WheelContents = { convert: [], tools: [], actions: [] };
const GROUPS: WheelGroup[] = ['convert', 'tools', 'actions'];

export default function WheelWindow() {
  const [paths, setPaths] = createSignal<string[]>([]);
  const [category, setCategory] = createSignal<string | null>(null);
  const [contents, setContents] = createSignal<WheelContents>(EMPTY);
  const [group, setGroup] = createSignal<WheelGroup>('convert');
  const [selected, setSelected] = createSignal(0);
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [status, setStatus] = createSignal('Waiting for a file');
  const [visible, setVisible] = createSignal(false);
  const [busy, setBusy] = createSignal(false);

  const segments = createMemo<Segment[]>(() => {
    const list = contents()[group()] ?? [];
    if (list.length > 0) return list;
    return [{ id: '__empty', label: 'No conversion for this file', disabled: true }];
  });

  const ring = createMemo<RingGeometry>(() =>
    layoutRing(segments().length, {
      cx: CX,
      cy: CY,
      innerRadius: INNER,
      outerRadius: OUTER,
      // The gap is applied in degrees per wedge, so it stays even at any count.
      gapPx: 0,
    }),
  );

  const fontSize = createMemo(() => {
    // Sizes are in viewBox units, which render at ~0.88 px here: 17 units ≈ 15 px,
    // the floor for a label read at a glance with the pointer already moving.
    const count = segments().length;
    if (count <= 6) return 22;
    if (count <= 8) return 20;
    if (count <= 10) return 18;
    return 17;
  });

  const clamped = createMemo(() => Math.min(selected(), Math.max(0, segments().length - 1)));
  const active = createMemo<Segment | null>(() => segments()[clamped()] ?? null);

  /** 0 → 1 while the selected wedge grows. Driven by rAF, not CSS transitions:
   *  CSS transforms on SVG groups are exactly what engines disagree about. */
  const [lift, setLift] = createSignal(1);
  let tweened = false;
  createEffect(() => {
    clamped();
    // The first painted selection is already grown: opening the wheel must not
    // animate, both because it looks wrong and because it costs frame budget.
    if (!tweened) {
      tweened = true;
      return;
    }
    const started = performance.now();
    let frame = requestAnimationFrame(function step(now) {
      const t = Math.min(1, (now - started) / 200);
      setLift(1 - Math.pow(1 - t, 3));
      if (t < 1) frame = requestAnimationFrame(step);
    });
    onCleanup(() => cancelAnimationFrame(frame));
  });

  /** Stable order, selected last: SVG has no z-index, so the grown wedge is painted
   *  above its neighbours and its edges cross them. */
  const painted = createMemo<SectorGeometry[]>(() => {
    const sectors = ring().sectors;
    const sel = clamped();
    return [
      ...sectors.filter((sector) => sector.index !== sel),
      ...sectors.filter((sector) => sector.index === sel),
    ];
  });

  let pendingPointer: { x: number; y: number } | null = null;
  let lastPointer: { x: number; y: number } | null = null;
  let frameId = 0;
  let dismissalTimer: number | undefined;
  let applyToken = 0;

  function stageMetrics(): { cx: number; cy: number; scale: number } {
    const size = Math.min(VIEW, Math.min(window.innerWidth, window.innerHeight) * STAGE_FRACTION);
    return { cx: window.innerWidth / 2, cy: window.innerHeight / 2, scale: size / VIEW };
  }

  function select(index: number): void {
    const count = segments().length;
    if (count === 0) return;
    const wrapped = ((index % count) + count) % count;
    if (wrapped !== selected()) setSelected(wrapped);
  }

  function switchGroup(next: WheelGroup): void {
    if (group() === next) return;
    setGroup(next);
    setSelected(0);
  }

  async function applyPaths(next: string[], keepGroup = false): Promise<void> {
    if (next.length === 0) return;
    const token = ++applyToken;
    setPaths(next);
    setStatus(`Reading ${next.length} file${next.length === 1 ? '' : 's'}`);
    const analysis = await ipc.analyze(next).catch(() => null);
    if (token !== applyToken) return;
    const detected = analysis?.category ?? null;
    setCategory(detected);
    let data = EMPTY;
    if (detected) {
      data = await ipc.wheels(detected, analysis?.format ?? null).catch(() => EMPTY);
      if (token !== applyToken) return;
    }
    setContents(data);
    const current = group();
    const preferred: WheelGroup =
      keepGroup && data[current].length > 0
        ? current
        : data.convert.length > 0
          ? 'convert'
          : data.tools.length > 0
            ? 'tools'
            : 'convert';
    setGroup(preferred);
    setSelected(0);
    setVisible(true);
    const skipped = analysis?.skipped.length ?? 0;
    setStatus(
      [
        `${next.length} file${next.length === 1 ? '' : 's'}`,
        detected ? categoryLabel(detected) : 'Mixed selection',
        skipped > 0 ? `${skipped} skipped` : null,
      ]
        .filter((part): part is string => part !== null)
        .join(' \u00b7 '),
    );
  }

  function scheduleDismissal(): void {
    if (ipc.isMock() || dismissalTimer !== undefined) return;
    dismissalTimer = window.setTimeout(() => {
      dismissalTimer = undefined;
      void dismiss();
    }, 400);
  }

  function clearDismissal(): void {
    if (dismissalTimer !== undefined) {
      window.clearTimeout(dismissalTimer);
      dismissalTimer = undefined;
    }
  }

  function localPoint(pointer: { x: number; y: number }): { x: number; y: number } {
    const { cx, cy, scale } = stageMetrics();
    return { x: CX + (pointer.x - cx) / scale, y: CY + (pointer.y - cy) / scale };
  }

  function pointerIndex(pointer: { x: number; y: number }): number {
    const local = localPoint(pointer);
    return indexForPoint(local.x, local.y, ring(), CX, CY, INNER - GROW_IN, OUTER + GROW_OUT);
  }

  function flushPointer(): void {
    frameId = 0;
    const pointer = pendingPointer;
    if (!pointer || !visible()) return;
    pendingPointer = null;
    const local = localPoint(pointer);
    const index = indexForPoint(local.x, local.y, ring(), CX, CY, INNER - GROW_IN, OUTER + GROW_OUT);
    if (index >= 0 && index !== clamped()) setSelected(index);
    if (Math.hypot(local.x - CX, local.y - CY) > OUTER + GROW_OUT + 18) scheduleDismissal();
    else clearDismissal();
  }

  function onPointerMove(event: PointerEvent): void {
    if (!visible()) return;
    lastPointer = { x: event.clientX, y: event.clientY };
    pendingPointer = lastPointer;
    if (frameId === 0) frameId = requestAnimationFrame(flushPointer);
  }

  async function dismiss(): Promise<void> {
    clearDismissal();
    setVisible(false);
    try {
      await ipc.wheelHide();
    } catch {
      // The window may already be gone; dismissal still stands.
    }
  }

  async function commit(): Promise<void> {
    const segment = active();
    const currentCategory = category();
    const files = paths();
    if (busy() || !segment || segment.disabled || !currentCategory || files.length === 0) return;
    setBusy(true);
    const groupAtCommit = group();
    try {
      if (groupAtCommit === 'actions') {
        await performAction(segment.id, files);
      } else if (groupAtCommit === 'tools') {
        await ipc.run(files, currentCategory, { kind: 'tool', tool: segment.id, params: {} });
      } else {
        await ipc.run(files, currentCategory, { kind: 'convert', target: segment.id });
      }
      setStatus(
        `${segment.label} started for ${files.length} file${files.length === 1 ? '' : 's'}`,
      );
      await dismiss();
    } catch (error) {
      setStatus(error instanceof Error ? error.message : 'The backend rejected the request');
    } finally {
      setBusy(false);
    }
  }

  function onKeyDown(event: KeyboardEvent): void {
    switch (event.key) {
      case 'ArrowLeft':
      case 'ArrowUp':
        event.preventDefault();
        select(clamped() - 1);
        break;
      case 'ArrowRight':
      case 'ArrowDown':
        event.preventDefault();
        select(clamped() + 1);
        break;
      case 'Enter':
      case ' ':
        event.preventDefault();
        void commit();
        break;
      case 'Escape':
        event.preventDefault();
        void dismiss();
        break;
      default: {
        const key = wheelKeyFromEventKey(event.key);
        if (key && !event.repeat) {
          const target = groupForKey(key, settings());
          if (target) switchGroup(target);
        }
        break;
      }
    }
  }

  function applyMode(mode: string): void {
    const normalized = mode.toLowerCase();
    if (normalized === 'convert' || normalized === 'tools' || normalized === 'actions') {
      switchGroup(normalized);
      return;
    }
    const target = groupForKey(mode, settings());
    if (target) switchGroup(target);
  }

  /**
   * A drop on the wheel window either finishes a wheel that is already showing
   * (commit the wedge under the cursor) or, for the drop overlay, supplies the files
   * and then raises the wheel.
   */
  async function handleDrop(next: string[]): Promise<void> {
    if (next.length === 0) return;
    const wasVisible = visible();
    await applyPaths(next, wasVisible);
    if (!wasVisible || ipc.isMock() || !lastPointer) return;
    const index = pointerIndex(lastPointer);
    if (index < 0) return;
    setSelected(index);
    void commit();
  }

  onMount(() => {
    document.documentElement.dataset.window = 'wheel';
    document.body.classList.add('wheel-body');

    const unlisteners: UnlistenFn[] = [];
    void ipc.boot().then(
      (info) => setSettings(info.settings),
      () => setSettings(null),
    );
    void ipc
      .onEvent('open-paths', (next) => {
        void applyPaths(next);
      })
      .then((unlisten) => unlisteners.push(unlisten));
    void ipc
      .onEvent('drag-signal', (signal) => {
        if (signal.kind === 'modifier' && signal.active) applyMode(signal.key);
      })
      .then((unlisten) => unlisteners.push(unlisten));
    void ipc
      .onEvent('wheel-mode', (mode) => applyMode(mode))
      .then((unlisten) => unlisteners.push(unlisten));
    void ipc
      .onEvent('wheel-shown', () => {
        if (paths().length > 0) setVisible(true);
        else setStatus('Drop a file or choose a target');
      })
      .then((unlisten) => unlisteners.push(unlisten));
    void ipc
      .onEvent('wheel-hidden', () => {
        setVisible(false);
        clearDismissal();
      })
      .then((unlisten) => unlisteners.push(unlisten));
    void ipc
      .onEvent('overlay-open', () => {
        setVisible(false);
        setStatus('Drop the file here to choose a target');
      })
      .then((unlisten) => unlisteners.push(unlisten));
    void ipc
      .subscribeDragDrop((payload) => {
        if (payload.kind === 'drop') void handleDrop(payload.paths);
      })
      .then((unlisten) => unlisteners.push(unlisten));
    // Perf handshake: answer only after the frame carrying the wheel has painted.
    void ipc
      .onAnyEvent('perf-ping', (payload) => {
        const seq = typeof payload === 'number' ? payload : 0;
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            void ipc.emitToBackend('perf-pong', seq);
          });
        });
      })
      .then((unlisten) => unlisteners.push(unlisten));

    document.addEventListener('keydown', onKeyDown);
    window.addEventListener('pointermove', onPointerMove, { passive: true });

    if (ipc.isMock()) {
      void ipc.pickFiles().then((demo) => {
        if (demo.length > 0) void applyPaths(demo);
      });
    }

    onCleanup(() => {
      applyToken += 1;
      document.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('pointermove', onPointerMove);
      if (frameId !== 0) cancelAnimationFrame(frameId);
      clearDismissal();
      for (const unlisten of unlisteners) unlisten();
      delete document.documentElement.dataset.window;
      document.body.classList.remove('wheel-body');
    });
  });

  /** Radius the icon+label pair is balanced around, in the middle of the band. */
  function pairRadius(): number {
    return INNER + (OUTER - INNER) * 0.55;
  }

  function labelRadius(): number {
    return pairRadius();
  }

  /**
   * Point on the wedge's bisector that the icon+label *block* is balanced around.
   * Stacking the glyph in screen space shifts the pair sideways on the left and right
   * segments unless the block, not the label, is what gets centred.
   */
  function pairCentre(sector: SectorGeometry): { x: number; y: number } {
    return pointAt(sector, labelRadius() + (selected() === sector.index ? 4 * lift() : 0));
  }

  /** The glyph always sits above the label, whatever the sector's angle. */
  function iconPoint(sector: SectorGeometry): { x: number; y: number } {
    const p = pairCentre(sector);
    return { x: p.x, y: p.y - 10 };
  }


  function pointAt(sector: SectorGeometry, radius: number): { x: number; y: number } {
    return {
      x: CX + radius * Math.cos(sector.midAngle),
      y: CY + radius * Math.sin(sector.midAngle),
    };
  }

  function labelPoint(sector: SectorGeometry): { x: number; y: number } {
    const p = pairCentre(sector);
    return { x: p.x, y: p.y + 16 };
  }

  /** The glyph is authored on a 24-unit grid; scale it into the wedge band. */
  function iconScale(): number {
    // Glyphs are authored between 4 and 20 on the 24-unit grid, so they occupy 16
    // units: 16 * 1.4 ≈ 22 px on screen at the default window size.
    const count = segments().length;
    if (count <= 6) return 1.6;
    if (count <= 8) return 1.5;
    if (count <= 10) return 1.45;
    return 1.4;
  }

  /**
   * Fit a label to the chord its wedge actually offers. The size steps down from the
   * segment-count size until the text fits, then truncates if it still does not — a
   * six-segment tools wheel has room for "Edit metadata" at 20px, a twelve-segment
   * convert wheel does not, and guessing one size for both overflows the wedge.
   */
  function sectorText(sector: SectorGeometry): { text: string; size: number } {
    const segment = segments()[sector.index];
    if (!segment) return { text: '', size: fontSize() };
    const chord = 2 * labelRadius() * Math.sin(Math.min(Math.PI, ring().step) / 2) - 14;
    const widthOf = (text: string, size: number) => text.length * size * 0.66;
    const cap = fontSize();
    let size = cap;
    while (size > 12 && widthOf(segment.label, size) > chord) size -= 1;
    const label = segment.label;
    if (widthOf(label, size) <= chord) return { text: label, size };
    const max = Math.max(3, Math.floor(chord / (size * 0.66)));
    return { text: `${label.slice(0, max - 1)}…`, size };
  }

  /** 0 → 1 for the selected wedge, eased by the rAF tween. */
  function growth(sector: SectorGeometry): number {
    return selected() === sector.index ? lift() : 0;
  }

  /**
   * The wedge at its current growth. A selected wedge widens sideways, deepens inward
   * and reaches outward — the three moves together are what make it read as floating
   * above the ring rather than merely recoloured.
   */
  function wedgePath(sector: SectorGeometry, extra = 0): string {
    const amount = growth(sector);
    const gap = ((GAP_DEG * Math.PI) / 180) / 2;
    const spread = ((SPREAD_DEG * Math.PI) / 180) * amount;
    const inner = INNER - GROW_IN * amount;
    const outer = OUTER + GROW_OUT * amount + extra;
    return radialWedgePath(
      CX,
      CY,
      inner,
      outer,
      sector.startAngle + gap - spread,
      sector.endAngle - gap + spread,
      CORNER,
    );
  }

  const hint = (): string => {
    const current = settings();
    return [
      `${keyLabel(current?.convert_key ?? 'shift')} convert`,
      `${keyLabel(current?.tools_key ?? 'control')} tools`,
      `${keyLabel(current?.actions_key ?? 'alt')} actions`,
      '\u2190\u2192 select',
      'Enter run',
      'Esc close',
    ].join(' \u00b7 ');
  };

  return (
    <div class="wheel-window">
      <Show
        when={visible()}
        fallback={
          <div class="wheel-waiting" role="status">
            <p class="wheel-waiting-title">Wheel is waiting for a drop</p>
            <p class="wheel-waiting-copy">{status()}</p>
          </div>
        }
      >
        <div class="wheel-stage">
          <svg
            class="wheel-svg"
            viewBox={`0 0 ${VIEW} ${VIEW}`}
            role="group"
            aria-label="Conversion wheel"
          >
            <defs>
              <linearGradient id="petalFace" gradientUnits="userSpaceOnUse" x1="0" y1="20" x2="0" y2="620">
                <stop offset="0%" stop-color="var(--petal-top)" />
                <stop offset="55%" stop-color="var(--petal-mid)" />
                <stop offset="100%" stop-color="var(--petal-bottom)" />
              </linearGradient>
            </defs>

            <circle class="wheel-plate" cx={CX} cy={CY} r={PLATE_R} />

            <For each={painted()}>
              {(sector) => {
                const segment = (): Segment | undefined => segments()[sector.index];
                const isSelected = (): boolean => selected() === sector.index;
                return (
                  <g
                    class="wheel-seg"
                    data-id={segment()?.id}
                    classList={{
                      'is-selected': isSelected(),
                      'is-disabled': segment()?.disabled === true,
                    }}
                  >
                    <Show when={isSelected()}>
                      <path
                        class="wheel-seg-copy"
                        d={wedgePath(sector, 2)}
                        transform={`translate(0 ${(COPY_DROP * growth(sector)).toFixed(2)})`}
                        opacity={0.28 * growth(sector)}
                      />
                    </Show>
                    <path
                      class="wheel-seg-shape"
                      d={wedgePath(sector)}
                      onClick={() => {
                        select(sector.index);
                        void commit();
                      }}
                    >
                      <title>{segment()?.blurb ?? segment()?.label ?? ''}</title>
                    </path>
                    <g
                      class="wheel-seg-icon"
                      transform={`translate(${iconPoint(sector).x} ${iconPoint(sector).y}) scale(${iconScale()}) translate(-12 -12)`}
                    >
                      <path d={glyphFor(segment()?.id ?? '')} />
                    </g>
                    <text
                      class="wheel-seg-label"
                      x={labelPoint(sector).x}
                      y={labelPoint(sector).y}
                      text-anchor="middle"
                      dominant-baseline="central"
                      font-size={String(sectorText(sector).size)}
                    >
                      {sectorText(sector).text}
                    </text>
                  </g>
                );
              }}
            </For>


          </svg>

          <div class="wheel-center" aria-live="polite">
            <div class="wheel-hub">
              <div class="wheel-modes" role="tablist" aria-label="Wheel mode">
                <For each={GROUPS}>
                  {(mode) => (
                    <button
                      type="button"
                      role="tab"
                      class="wheel-mode"
                      classList={{ 'is-active': group() === mode }}
                      aria-selected={group() === mode}
                      onClick={() => switchGroup(mode)}
                    >
                      {mode === 'convert' ? 'Convert' : mode === 'tools' ? 'Tools' : 'Actions'}
                    </button>
                  )}
                </For>
              </div>
              <p class="wheel-center-label">{active()?.label ?? ''}</p>
              <p class="wheel-center-count">
                {paths().length} file{paths().length === 1 ? '' : 's'}
              </p>
            </div>
          </div>
        </div>
      </Show>

      <footer class="wheel-footer">
        <p class="wheel-hint">{hint()}</p>
        <p class="wheel-status" role="status">
          {status()}
        </p>
      </footer>
    </div>
  );
}
