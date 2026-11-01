import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import * as ipc from '../ipc';
import { collageCellBox } from '../geometry';
import type { CollageCellLayout, CollageLayout } from '../types';
import './collage-preview.css';

/** One picture's crop, as the engine takes it: source index, zoom and pan. */
export interface CollageCell {
  index: number;
  zoom: number;
  x: number;
  y: number;
}

interface CollagePreviewProps {
  paths: string[];
  /** The flattened parameters the job will receive. */
  params: Record<string, unknown>;
  /** The transforms travel through the modal's extras, not through `collected()`. */
  onCells?: (cells: CollageCell[]) => void;
  /** Bumped by the modal's Reset; the pan and zoom start over with the fields. */
  resetToken?: number;
}

interface Drag {
  pointerId: number;
  index: number;
  startX: number;
  startY: number;
  panX: number;
  panY: number;
  width: number;
  height: number;
}

interface LiveSource {
  url: string;
  w: number;
  h: number;
}

const clamp = (value: number, min: number, max: number): number =>
  Math.min(max, Math.max(min, value));

/**
 * How long to wait for the pointer to pause before asking the engine again. The
 * composer is the authority, but it costs a few hundred milliseconds; the live layer
 * (below) is what moves under the pointer meanwhile, so this timer can stay short
 * without queueing one process per pointer event.
 */
const RENDER_DELAY = 120;

/** A stored transform array, ignoring entries that are not usable numbers. */
function readCells(raw: unknown): CollageCell[] {
  if (!Array.isArray(raw)) return [];
  const cells: CollageCell[] = [];
  for (const entry of raw) {
    if (typeof entry !== 'object' || entry === null) continue;
    const value = entry as Record<string, unknown>;
    const index = Number(value.index);
    if (!Number.isFinite(index) || index < 0) continue;
    const zoom = Number(value.zoom);
    const x = Number(value.x);
    const y = Number(value.y);
    cells.push({
      index: Math.round(index),
      zoom: Number.isFinite(zoom) ? clamp(zoom, 1, 4) : 1,
      x: Number.isFinite(x) ? clamp(x, -100, 100) : 0,
      y: Number.isFinite(y) ? clamp(y, -100, 100) : 0,
    });
  }
  return cells;
}

/**
 * The composition as the engine will write it, with a hit region per placed picture.
 *
 * Two layers: a live crop of the selected picture (a 512 px thumbnail placed by the
 * same formula the composer uses), which follows the pointer and the zoom slider
 * immediately; and the engine's rendered composition, requested at most once at a
 * time. The live crop is hidden as soon as a render that includes the current
 * transforms lands, so the dialog never shows a lookalike for longer than a render.
 *
 * A request that arrives while another is in flight is remembered and started when
 * that one finishes. The old debounce restarted on every pointer move, so a drag
 * produced no render at all until the pointer stopped — the report read as "it only
 * updates after I finish".
 */
export default function CollagePreview(props: CollagePreviewProps) {
  const [src, setSrc] = createSignal('');
  const [layout, setLayout] = createSignal<CollageLayout | null>(null);
  const [status, setStatus] = createSignal('');
  const [cells, setCells] = createSignal<CollageCell[]>(readCells(props.params.cells));
  const [selected, setSelected] = createSignal<number | null>(null);
  const [boxWidth, setBoxWidth] = createSignal(640);
  const [live, setLive] = createSignal(false);
  // Bumped when a thumbnail finishes loading, so the live style recomputes.
  const [thumbTick, setThumbTick] = createSignal(0);
  let host: HTMLDivElement | undefined;
  let frame: HTMLDivElement | undefined;
  let drag: Drag | null = null;
  let timer: number | undefined;
  let token = 0;
  let inflight = false;
  let pending = false;
  let lastStarted = '';
  let lastReset = props.resetToken ?? 0;
  const thumbs = new Map<number, LiveSource>();

  const fit = (): string => String(props.params.fit ?? 'crop');

  const cellFor = (index: number): CollageCell =>
    cells().find((cell) => cell.index === index) ?? { index, zoom: 1, x: 0, y: 0 };

  const readout = (): string => {
    const count = props.paths.length;
    if (count === 0) return 'Select images to preview the collage.';
    const current = layout();
    const n = clamp((selected() ?? 0) + 1, 1, count);
    const arrangement = current ? `${current.cols} \u00d7 ${current.rows}` : '';
    const action =
      fit() === 'contain'
        ? 'pan is off while Fit inside cells is on'
        : 'drag to move, zoom to fill';
    return `Picture ${n} of ${count} \u2014 ${action}${arrangement ? ` \u00b7 ${arrangement} cells` : ''}`;
  };

  const frameSize = createMemo(() => {
    const current = layout();
    if (!current || current.width <= 0 || current.height <= 0) return null;
    const scale = Math.min(Math.max(boxWidth(), 120) / current.width, 260 / current.height);
    return { w: Math.round(current.width * scale), h: Math.round(current.height * scale) };
  });

  const frameStyle = (): Record<string, string> => {
    const size = frameSize();
    return size ? { width: `${size.w}px`, height: `${size.h}px` } : {};
  };

  const cellStyle = (cell: CollageCellLayout): Record<string, string> => {
    const current = layout();
    if (!current) return {};
    return {
      left: `${(cell.x / current.width) * 100}%`,
      top: `${(cell.y / current.height) * 100}%`,
      width: `${(cell.w / current.width) * 100}%`,
      height: `${(cell.h / current.height) * 100}%`,
    };
  };

  const selectedCell = createMemo<CollageCellLayout | null>(() => {
    const current = layout();
    const index = selected();
    if (!current || index === null) return null;
    return current.cells.find((cell) => cell.input === index) ?? null;
  });

  const liveSource = (): LiveSource | undefined => {
    thumbTick();
    const index = selected();
    return index === null ? undefined : thumbs.get(index);
  };

  /**
   * The selected picture's crop, in display pixels of its cell. The composer's own
   * formula (`collageCellBox`) decides it, so the live layer cannot drift from the
   * arrangement the dialog will save.
   */
  const liveStyle = (): Record<string, string> | null => {
    const cell = selectedCell();
    const size = frameSize();
    const current = layout();
    const source = liveSource();
    if (!cell || !size || !current || !source) return null;
    const cellW = (cell.w / current.width) * size.w;
    const cellH = (cell.h / current.height) * size.h;
    const transform = cellFor(cell.input);
    const box = collageCellBox(
      cellW,
      cellH,
      source.w,
      source.h,
      transform.zoom,
      transform.x,
      transform.y,
      fit() === 'contain' ? 'contain' : 'crop',
    );
    if (!box) return null;
    return {
      width: `${box.width.toFixed(2)}px`,
      height: `${box.height.toFixed(2)}px`,
      left: `${box.left.toFixed(2)}px`,
      top: `${box.top.toFixed(2)}px`,
    };
  };

  /** The live crop's corners must match the engine's radius for this cell. */
  const liveRadius = (): string => {
    const cell = selectedCell();
    const size = frameSize();
    const current = layout();
    if (!cell || !size || !current) return '0px';
    const cellW = (cell.w / current.width) * size.w;
    const cellH = (cell.h / current.height) * size.h;
    const pct = clamp(Number(props.params.radius_pct ?? 0), 0, 100);
    return `${((Math.min(cellW, cellH) * pct) / 200).toFixed(1)}px`;
  };

  function ensureThumb(index: number): void {
    if (thumbs.has(index)) return;
    const path = props.paths[index];
    if (!path) return;
    void ipc
      .previewThumb(path, 512)
      .then((url) => {
        const image = new Image();
        image.onload = () => {
          thumbs.set(index, {
            url: ipc.assetUrl(url),
            w: image.naturalWidth || 1000,
            h: image.naturalHeight || 1000,
          });
          setThumbTick((tick) => tick + 1);
        };
        image.src = ipc.assetUrl(url);
      })
      .catch(() => {
        // No thumbnail: the composed preview still shows the saved arrangement.
      });
  }

  function updateCell(index: number, patch: Partial<CollageCell>): void {
    const next = cells().slice();
    const at = next.findIndex((cell) => cell.index === index);
    const current = at >= 0 ? next[at] : { index, zoom: 1, x: 0, y: 0 };
    const merged = { ...current, ...patch };
    const cell: CollageCell = {
      index,
      zoom: clamp(Number.isFinite(merged.zoom) ? merged.zoom : 1, 1, 4),
      x: clamp(Number.isFinite(merged.x) ? merged.x : 0, -100, 100),
      y: clamp(Number.isFinite(merged.y) ? merged.y : 0, -100, 100),
    };
    if (at >= 0) next[at] = cell;
    else next.push(cell);
    setCells(next);
    setLive(true);
    props.onCells?.(next);
  }

  function resetCell(index: number): void {
    const next = cells().filter((cell) => cell.index !== index);
    setCells(next);
    setLive(true);
    props.onCells?.(next);
  }

  function beginDrag(event: PointerEvent, cell: CollageCellLayout): void {
    setSelected(cell.input);
    ensureThumb(cell.input);
    if (fit() === 'contain') return;
    const current = layout();
    const box = frame?.getBoundingClientRect();
    if (!current || !box || box.width <= 0 || box.height <= 0) return;
    const transform = cellFor(cell.input);
    drag = {
      pointerId: event.pointerId,
      index: cell.input,
      startX: event.clientX,
      startY: event.clientY,
      panX: transform.x,
      panY: transform.y,
      width: (cell.w / current.width) * box.width,
      height: (cell.h / current.height) * box.height,
    };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function moveDrag(event: PointerEvent): void {
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (drag.width <= 0 || drag.height <= 0) return;
    // The picture follows the pointer: moving it right reveals its left edge, which
    // means the crop window moves left.
    updateCell(drag.index, {
      x: drag.panX - ((event.clientX - drag.startX) / drag.width) * 100,
      y: drag.panY - ((event.clientY - drag.startY) / drag.height) * 100,
    });
  }

  function endDrag(event: PointerEvent): void {
    if (!drag || drag.pointerId !== event.pointerId) return;
    drag = null;
    const target = event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) {
      target.releasePointerCapture(event.pointerId);
    }
  }

  function schedule(delay: number): void {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void render(), delay);
  }

  async function render(): Promise<void> {
    const paths = [...props.paths];
    if (paths.length === 0) return;
    if (inflight) {
      pending = true;
      return;
    }
    const request = JSON.stringify({ ...props.params, cells: cells() });
    if (request === lastStarted) return;
    lastStarted = request;
    inflight = true;
    pending = false;
    const mine = ++token;
    setStatus('Rendering…');
    try {
      const payload = JSON.parse(request) as Record<string, unknown>;
      const [path, next] = await Promise.all([
        ipc.collagePreview(paths, payload),
        ipc.collageLayout(paths, payload),
      ]);
      if (mine === token) {
        setSrc(ipc.assetUrl(path));
        setLayout(next);
        setStatus('');
        // The composed image now contains the transforms that are live. A newer
        // request invalidates this one, so the live crop stays until then.
        if (JSON.stringify({ ...props.params, cells: cells() }) === request) setLive(false);
      }
    } catch (error) {
      if (mine === token) {
        setStatus(error instanceof Error ? error.message : 'Preview unavailable.');
      }
    } finally {
      inflight = false;
      if (pending) schedule(0);
    }
  }

  onMount(() => {
    if (!host) return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? 0;
      if (width > 0) setBoxWidth(width);
    });
    observer.observe(host);
    onCleanup(() => observer.disconnect());
  });

  // Reset puts the transforms back where the tool started, without touching the
  // other fields the modal owns.
  createEffect(() => {
    const current = props.resetToken ?? 0;
    if (current === lastReset) return;
    lastReset = current;
    setCells(readCells(props.params.cells));
    setSelected(null);
    setLive(false);
    drag = null;
  });

  createEffect(() => {
    const count = props.paths.length;
    const current = selected();
    if (current !== null && current >= count) setSelected(null);
  });

  createEffect(() => {
    const paths = [...props.paths];
    // Every field and every transform is part of the request: a pan must re-render.
    const request = JSON.stringify({ ...props.params, cells: cells() });
    window.clearTimeout(timer);
    pending = false;
    if (paths.length === 0) {
      setSrc('');
      setLayout(null);
      setStatus('');
      return;
    }
    if (request === lastStarted) {
      // The composed image already contains this request (a drag that returned to the
      // last rendered transform): nothing to render, and no live crop left over.
      setLive(false);
      return;
    }
    pending = true;
    schedule(RENDER_DELAY);
  });

  onCleanup(() => window.clearTimeout(timer));

  return (
    <div class="collage-preview" ref={host}>
      <Show when={src()} fallback={<p class="collage-empty">{status() || 'Nothing to preview yet.'}</p>}>
        <div class="collage-frame" style={frameStyle()} ref={frame}>
          <img src={src()} alt="Collage preview" draggable={false} />
          <Show when={live() && selectedCell() && liveStyle()}>
            <div
              class="collage-live"
              style={{ ...cellStyle(selectedCell()!), 'border-radius': liveRadius() }}
            >
              <img
                src={liveSource()?.url}
                alt=""
                aria-hidden="true"
                draggable={false}
                style={liveStyle() ?? {}}
              />
            </div>
          </Show>
          <Show when={layout()}>
            <For each={layout()?.cells ?? []}>
              {(cell) => (
                <button
                  type="button"
                  class="collage-cell"
                  classList={{ 'is-selected': selected() === cell.input }}
                  style={cellStyle(cell)}
                  aria-label={`Cell ${cell.input + 1}`}
                  aria-pressed={selected() === cell.input}
                  onPointerDown={(event) => beginDrag(event, cell)}
                  onPointerMove={moveDrag}
                  onPointerUp={endDrag}
                  onPointerCancel={endDrag}
                />
              )}
            </For>
          </Show>
        </div>
      </Show>
      <Show when={status()}>
        <p class="collage-badge" role="status">
          {status()}
        </p>
      </Show>
      <p class="collage-readout">{readout()}</p>
      <div class="collage-controls">
        <label class="collage-zoom" for="collage-zoom">
          Zoom
        </label>
        <input
          id="collage-zoom"
          type="range"
          min="1"
          max="4"
          step="0.05"
          aria-label="Zoom"
          disabled={selected() === null || fit() === 'contain'}
          value={cellFor(selected() ?? 0).zoom}
          onInput={(event) => {
            const index = selected();
            if (index === null) return;
            ensureThumb(index);
            updateCell(index, { zoom: Number(event.currentTarget.value) });
          }}
        />
        <button
          type="button"
          class="collage-reset"
          aria-label="Reset image"
          disabled={selected() === null || !cells().some((cell) => cell.index === selected())}
          onClick={() => {
            const index = selected();
            if (index !== null) resetCell(index);
          }}
        >
          Reset image
        </button>
      </div>
    </div>
  );
}
