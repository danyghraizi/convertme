import { For, Show, createEffect, createSignal, onCleanup, onMount } from 'solid-js';
import * as ipc from '../ipc';
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

const clamp = (value: number, min: number, max: number): number =>
  Math.min(max, Math.max(min, value));

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
 * The render is debounced, and a stale response is dropped rather than paired with
 * newer settings. The previous picture stays on screen while the next one renders,
 * and the "Rendering…" badge is overlaid: transient text must never push the fields
 * below it up or down.
 *
 * The layout comes from the backend, not from a second grid implementation here, so
 * dragging a cell edits the same picture the composer will crop.
 */
export default function CollagePreview(props: CollagePreviewProps) {
  const [src, setSrc] = createSignal('');
  const [layout, setLayout] = createSignal<CollageLayout | null>(null);
  const [status, setStatus] = createSignal('');
  const [cells, setCells] = createSignal<CollageCell[]>(readCells(props.params.cells));
  const [selected, setSelected] = createSignal<number | null>(null);
  const [boxWidth, setBoxWidth] = createSignal(640);
  let host: HTMLDivElement | undefined;
  let frame: HTMLDivElement | undefined;
  let drag: Drag | null = null;
  let timer: number | undefined;
  let token = 0;
  let lastReset = props.resetToken ?? 0;

  const cellFor = (index: number): CollageCell =>
    cells().find((cell) => cell.index === index) ?? { index, zoom: 1, x: 0, y: 0 };

  const readout = (): string => {
    const count = props.paths.length;
    if (count === 0) return 'Select images to preview the collage.';
    const n = clamp((selected() ?? 0) + 1, 1, count);
    return String(props.params.fit ?? 'crop') === 'contain'
      ? `Picture ${n} of ${count} — pan is off while Fit inside cells is on`
      : `Picture ${n} of ${count} — drag to move, zoom to fill`;
  };

  const frameStyle = (): Record<string, string> => {
    const current = layout();
    if (!current || current.width <= 0 || current.height <= 0) return {};
    const scale = Math.min(Math.max(boxWidth(), 120) / current.width, 260 / current.height);
    return {
      width: `${Math.round(current.width * scale)}px`,
      height: `${Math.round(current.height * scale)}px`,
    };
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
    props.onCells?.(next);
  }

  function resetCell(index: number): void {
    const next = cells().filter((cell) => cell.index !== index);
    setCells(next);
    props.onCells?.(next);
  }

  function beginDrag(event: PointerEvent, cell: CollageCellLayout): void {
    setSelected(cell.input);
    if (String(props.params.fit ?? 'crop') === 'contain') return;
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
    if (paths.length === 0) {
      setSrc('');
      setLayout(null);
      setStatus('');
      return;
    }
    timer = window.setTimeout(() => {
      const mine = ++token;
      setStatus('Rendering…');
      const payload = JSON.parse(request) as Record<string, unknown>;
      void Promise.all([
        ipc.collagePreview(paths, payload),
        ipc.collageLayout(paths, payload),
      ])
        .then(([path, next]) => {
          if (mine !== token) return;
          setSrc(ipc.assetUrl(path));
          setLayout(next);
          setStatus('');
        })
        .catch((error) => {
          if (mine !== token) return;
          setStatus(error instanceof Error ? error.message : 'Preview unavailable.');
        });
    }, 200);
  });

  onCleanup(() => window.clearTimeout(timer));

  return (
    <div class="collage-preview" ref={host}>
      <Show when={src()} fallback={<p class="collage-empty">{status() || 'Nothing to preview yet.'}</p>}>
        <div class="collage-frame" style={frameStyle()} ref={frame}>
          <img src={src()} alt="Collage preview" draggable={false} />
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
          disabled={selected() === null || String(props.params.fit ?? 'crop') === 'contain'}
          value={cellFor(selected() ?? 0).zoom}
          onInput={(event) => {
            const index = selected();
            if (index === null) return;
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
