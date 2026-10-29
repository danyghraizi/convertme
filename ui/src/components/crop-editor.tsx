import { Show, createSignal, onCleanup, onMount } from 'solid-js';
import * as ipc from '../ipc';
import type { Box } from '../types';

/**
 * The crop surface: the whole picture, with the region that will be kept, dragged by
 * its edges or corners. Sizes are shown in source pixels so the number in the field is
 * the one the file will have, not a percentage of a thumbnail.
 */
interface CropEditorProps {
  previewPath: string | null;
  /** Fractions 0..1, or null for "the whole image". */
  value: Box | null;
  /** `free` or `w:h`; the region keeps this ratio while it is dragged. */
  aspect: string;
  onChange: (box: Box) => void;
}

type Handle = 'move' | 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw' | 'new';

const MIN = 0.02;

export default function CropEditor(props: CropEditorProps) {
  const [preview, setPreview] = createSignal('');
  const [natural, setNatural] = createSignal({ w: 0, h: 0 });
  const [note, setNote] = createSignal('');
  const [draft, setDraft] = createSignal<Box | null>(null);
  let surface: HTMLDivElement | undefined;
  let rect: DOMRect | null = null;
  let anchor: { x: number; y: number } | null = null;
  let start: Box | null = null;
  let mode: Handle = 'new';

  const box = (): Box => draft() ?? props.value ?? { x: 0, y: 0, w: 1, h: 1 };

  onMount(() => {
    const path = props.previewPath;
    if (!path) {
      setNote('Preview unavailable; the crop can still be set from a rectangle.');
      return;
    }
    void ipc
      .previewThumb(path, 1024)
      .then((url) => {
        const image = new Image();
        image.onload = () => setNatural({ w: image.naturalWidth, h: image.naturalHeight });
        image.src = ipc.assetUrl(url);
        setPreview(ipc.assetUrl(url));
      })
      .catch(() => setNote('Preview unavailable.'));
  });

  onCleanup(() => setPreview(''));

  /** The locked width/height ratio, or null when the region is free. */
  function ratio(): number | null {
    const [a, b] = props.aspect.split(':').map(Number);
    if (!Number.isFinite(a) || !Number.isFinite(b) || a <= 0 || b <= 0) return null;
    // Fractions are of a non-square image, so the pixel ratio has to travel through it.
    const { w, h } = natural();
    if (w === 0 || h === 0) return null;
    return (a / b) * (h / w);
  }

  function clamp01(value: number): number {
    return Math.min(1, Math.max(0, value));
  }

  function point(event: PointerEvent): { x: number; y: number } {
    const r = rect ?? surface?.getBoundingClientRect();
    if (!r || r.width === 0) return { x: 0, y: 0 };
    return {
      x: clamp01((event.clientX - r.left) / r.width),
      y: clamp01((event.clientY - r.top) / r.height),
    };
  }

  function begin(event: PointerEvent, which: Handle): void {
    if (!surface) return;
    rect = surface.getBoundingClientRect();
    mode = which;
    anchor = point(event);
    start = { ...box() };
    surface.setPointerCapture(event.pointerId);
    if (which === 'new') setDraft({ x: anchor.x, y: anchor.y, w: 0, h: 0 });
    event.preventDefault();
    event.stopPropagation();
  }

  function resize(from: Box, which: Handle, p: { x: number; y: number }): Box {
    let { x, y, w, h } = from;
    const right = x + w;
    const bottom = y + h;
    if (which.includes('e')) w = clamp01(p.x) - x;
    if (which.includes('s')) h = clamp01(p.y) - y;
    if (which.includes('w')) {
      x = clamp01(Math.min(p.x, right - MIN));
      w = right - x;
    }
    if (which.includes('n')) {
      y = clamp01(Math.min(p.y, bottom - MIN));
      h = bottom - y;
    }
    w = Math.max(MIN, w);
    h = Math.max(MIN, h);
    x = Math.min(x, 1 - MIN);
    y = Math.min(y, 1 - MIN);
    w = Math.min(w, 1 - x);
    h = Math.min(h, 1 - y);
    const locked = ratio();
    if (locked !== null && which !== 'move') {
      // Grow the region from the corner that is not being dragged.
      const centreX = which.includes('w') ? x + w : x;
      const centreY = which.includes('n') ? y + h : y;
      if (w / locked > h) {
        h = w / locked;
        y = which.includes('n') ? centreY - h : centreY;
      } else {
        w = h * locked;
        x = which.includes('w') ? centreX - w : centreX;
      }
      x = Math.max(0, Math.min(x, 1 - w));
      y = Math.max(0, Math.min(y, 1 - h));
      if (x + w > 1) w = 1 - x;
      if (y + h > 1) h = 1 - y;
    }
    return { x, y, w, h };
  }

  function move(event: PointerEvent): void {
    if (!anchor || !start) return;
    const p = point(event);
    if (mode === 'move') {
      const dx = p.x - anchor.x;
      const dy = p.y - anchor.y;
      const x = Math.min(1 - start.w, Math.max(0, start.x + dx));
      const y = Math.min(1 - start.h, Math.max(0, start.y + dy));
      setDraft({ ...start, x, y });
      return;
    }
    const from = mode === 'new' ? { x: anchor.x, y: anchor.y, w: 0, h: 0 } : start;
    setDraft(resize(from, mode === 'new' ? 'se' : mode, p));
  }

  function end(event: PointerEvent): void {
    if (surface?.hasPointerCapture(event.pointerId)) {
      surface.releasePointerCapture(event.pointerId);
    }
    const rectBox = draft();
    anchor = null;
    start = null;
    setDraft(null);
    if (rectBox && rectBox.w >= MIN && rectBox.h >= MIN) {
      props.onChange(rectBox);
      setNote('');
    }
  }

  function reset(): void {
    props.onChange({ x: 0, y: 0, w: 1, h: 1 });
    setNote('');
  }

  const pixels = () => {
    const { w, h } = natural();
    const b = box();
    return {
      w: Math.round(b.w * w),
      h: Math.round(b.h * h),
      x: Math.round(b.x * w),
      y: Math.round(b.y * h),
    };
  };

  const handles: { id: Handle; style: Record<string, string> }[] = [
    { id: 'nw', style: { left: '0%', top: '0%' } },
    { id: 'n', style: { left: '50%', top: '0%' } },
    { id: 'ne', style: { left: '100%', top: '0%' } },
    { id: 'e', style: { left: '100%', top: '50%' } },
    { id: 'se', style: { left: '100%', top: '100%' } },
    { id: 's', style: { left: '50%', top: '100%' } },
    { id: 'sw', style: { left: '0%', top: '100%' } },
    { id: 'w', style: { left: '0%', top: '50%' } },
  ];

  return (
    <div class="crop-editor">
      <div
        class="crop-surface"
        classList={{ 'is-empty': preview().length === 0 }}
        ref={surface}
        onPointerDown={(event) => begin(event, 'new')}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
      >
        <Show when={preview()} fallback={<p class="crop-placeholder">Preview unavailable</p>}>
          <img src={preview()} alt="" draggable={false} />
          <span class="crop-shade" />
          <div
            class="crop-region"
            style={{
              left: `${box().x * 100}%`,
              top: `${box().y * 100}%`,
              width: `${box().w * 100}%`,
              height: `${box().h * 100}%`,
            }}
            onPointerDown={(event) =>
              // With the whole picture selected there is nothing to move; a drag
              // inside starts a new region instead, as the reference does.
              begin(event, box().w > 0.99 && box().h > 0.99 ? 'new' : 'move')
            }
            onPointerMove={move}
            onPointerUp={end}
            onPointerCancel={end}
          >
            {handles.map((handle) => (
              <span
                class={`crop-handle crop-handle-${handle.id}`}
                style={handle.style}
                onPointerDown={(event) => begin(event, handle.id)}
                onPointerMove={move}
                onPointerUp={end}
                onPointerCancel={end}
              />
            ))}
          </div>
        </Show>
      </div>
      <div class="crop-readout">
        <span>
          {pixels().w} × {pixels().h} px
        </span>
        <span class="crop-origin">
          at {pixels().x}, {pixels().y}
        </span>
        <button type="button" class="button button-quiet" onClick={reset}>
          Reset
        </button>
      </div>
      <Show when={note()}>
        <p class="field-help" role="status">
          {note()}
        </p>
      </Show>
    </div>
  );
}
