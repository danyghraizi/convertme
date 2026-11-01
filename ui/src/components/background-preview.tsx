import { Show, createEffect, createMemo, createSignal } from 'solid-js';
import * as ipc from '../ipc';
import { backgroundWindow as backgroundCrop } from '../geometry';
import './background-preview.css';

/**
 * The composition preview: the photo on its new canvas, with the padding, the corner
 * radius, the ratio and the fill it will be saved with. Every size is computed from the
 * numbers the engine gets, then scaled once to fit the panel, so the preview and the
 * file agree on proportions.
 *
 * The background picture is drawn from the same geometry the engine crops with: a cover
 * window of the picture at the canvas aspect, zoomed by `bg_zoom`, slid by `bg_x`/`bg_y`
 * and stretched to the canvas. CSS `background-size: cover` alone could not do that: it
 * fits the picture to the *element*, which is the canvas, and the zoom/placement then
 * drift as soon as the two aspects differ.
 */
interface BackgroundPreviewProps {
  path: string | null;
  mode: string;
  color: string;
  gradient: string;
  image: string | null;
  imageZoom: number;
  imageX: number;
  imageY: number;
  /** Percent of the shorter side, as the dialog shows them. */
  paddingPct: number;
  radiusPct: number;
  shadowPct: number;
  shadowAngle: number;
  /** Blur of the chosen background picture, in that picture's own pixels. */
  bgBlur: number;
  aspect: string;
  /** Drag on the canvas moves the chosen picture; the zoom slider sits under it. */
  onPlace?: (x: number, y: number) => void;
  onZoom?: (zoom: number) => void;
}

const RATIOS: Record<string, number> = {
  '1:1': 1,
  '4:3': 4 / 3,
  '16:9': 16 / 9,
  '9:16': 9 / 16,
  '3:4': 3 / 4,
  '4:5': 4 / 5,
};

const MAX_WIDTH = 430;
const MAX_HEIGHT = 200;
/**
 * The engine blurs the background picture in its own pixels, and a 6000 px picture
 * scaled down to a 400 px preview turns slider 50 into a fraction of a pixel. The
 * other way round, a tiny picture stretched to fill the panel would ask the
 * compositor for a 150 px blur on every mouse move. Cap what reaches CSS.
 */
const MAX_BLUR_PX = 32;

function finite(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, finite(value, low)));
}

export default function BackgroundPreview(props: BackgroundPreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('');
  const [natural, setNatural] = createSignal({ w: 4, h: 3 });
  const [real, setReal] = createSignal<{ w: number; h: number } | null>(null);
  const [backdrop, setBackdrop] = createSignal('');
  const [backdropSize, setBackdropSize] = createSignal<{ w: number; h: number } | null>(null);

  // Every load is tagged: a slow thumbnail for the previous file must not paint over
  // the current one after the arrows or the background changed.
  let photoTicket = 0;
  let backdropTicket = 0;

  // Reloads when the dialog switches file: the preview must follow the arrows.
  createEffect(() => {
    const path = props.path;
    const ticket = ++photoTicket;
    setReal(null);
    setSrc('');
    if (!path) {
      setNote('Preview unavailable; the settings still apply to the saved file.');
      return;
    }
    setNote('');
    void ipc
      .previewThumb(path, 1024)
      .then((url) => {
        if (ticket === photoTicket) setSrc(ipc.assetUrl(url));
      })
      .catch(() => {
        if (ticket === photoTicket) setNote('Preview unavailable.');
      });
    // The drawn bitmap is a thumbnail; the numbers must be the file's own pixels.
    void ipc
      .imageSize(path)
      .then((size) => {
        if (ticket === photoTicket && size) setReal(size);
      })
      .catch(() => {
        // The thumbnail's own size keeps the layout sane; nothing to report.
      });
  });

  const chosenImage = createMemo(() => props.image?.trim() ?? '');

  // The chosen background picture: its bitmap for the stage and its own pixel size for
  // the crop geometry. Both are keyed on the path; a stale answer is dropped.
  createEffect(() => {
    const path = chosenImage();
    const ticket = ++backdropTicket;
    setBackdrop('');
    setBackdropSize(null);
    if (!path) return;
    void ipc
      .previewThumb(path, 1024)
      .then((url) => {
        if (ticket === backdropTicket) setBackdrop(ipc.assetUrl(url));
      })
      .catch(() => {
        // Not a readable picture: the engine falls back to the solid colour too.
      });
    void ipc
      .imageSize(path)
      .then((size) => {
        if (ticket === backdropTicket) setBackdropSize(size);
      })
      .catch(() => {
        // Unknown size: the plain cover fallback draws instead of the exact fit.
      });
  });

  const fill = (): string => {
    if (props.mode === 'gradient') {
      const text = props.gradient || '';
      const parts = text.split('|');
      const stopsText = parts.length > 1 ? parts[parts.length - 1] : parts[0];
      const linear = !text.includes('radial');
      const angle = Number(parts.find((p) => Number.isFinite(Number(p)) && p.trim() !== '') ?? 0);
      const stops = (stopsText || '')
        .split(',')
        .map((entry) => {
          const [colour, position] = entry.split('@');
          return { colour: (colour || '#000').trim(), position: Number(position) };
        })
        .sort((a, b) => a.position - b.position)
        .map((stop) => `${stop.colour} ${Math.round(stop.position)}%`)
        .join(', ');
      return linear
        ? `linear-gradient(${Math.round(angle)}deg, ${stops})`
        : `radial-gradient(circle at 50% 50%, ${stops})`;
    }
    // The engine falls back to this colour when `mode` is image but no readable file
    // made it into the parameters, so the preview shows the same thing.
    return props.mode === 'colour' ? props.color || '#101418' : '#101418';
  };

  /** The canvas in source pixels: what the file will be. Mirrors `add_background`. */
  const canvasSize = createMemo(() => {
    const source = real() ?? natural();
    const w = Math.max(1, Math.round(finite(source.w, 4)));
    const h = Math.max(1, Math.round(finite(source.h, 3)));
    const pad = Math.round((clamp(props.paddingPct, 0, 100) / 100) * Math.min(w, h));
    let canvasW = w + 2 * pad;
    let canvasH = h + 2 * pad;
    const ratio = RATIOS[props.aspect];
    if (ratio) {
      if (canvasW / canvasH > ratio) canvasH = Math.max(1, Math.round(canvasW / ratio));
      else canvasW = Math.max(1, Math.round(canvasH * ratio));
    }
    return { w: canvasW, h: canvasH };
  });

  /** Canvas and photo sizes in source pixels, then one scale to fit the panel. */
  const layout = createMemo(() => {
    // Draw with the thumbnail's shape, but place the photo by the real proportions.
    const realSize = real();
    const { w, h } = realSize ?? natural();
    const pad = (clamp(props.paddingPct, 0, 100) / 100) * Math.min(w, h);
    const canvas = canvasSize();
    const scale = Math.min(MAX_WIDTH / canvas.w, MAX_HEIGHT / canvas.h);
    const photoW = w * scale;
    const photoH = h * scale;
    return {
      canvasW: canvas.w * scale,
      canvasH: canvas.h * scale,
      photoW,
      photoH,
      padPx: pad * scale,
      // 100% is a circle, so the slider maps to half the shorter side.
      radiusPx: Math.min(
        (props.radiusPct / 100) * (Math.min(w, h) / 2) * scale,
        Math.min(photoW, photoH) / 2,
      ),
      // Same numbers the engine uses: the shadow lives in the padding, sigma grows with
      // that, the offset is 0.9 sigma and the CSS blur radius about twice sigma.
      shadowPx: (props.shadowPct / 100) * pad * 0.8 * scale,
      shadowAlpha: 0.45 + 0.55 * (props.shadowPct / 100),
      shadowAngle: (props.shadowAngle * Math.PI) / 180,
    };
  });

  /**
   * The engine's window on the background picture, in that picture's own pixels.
   *
   * `base` is the cover crop: the largest window with the canvas's aspect. The zoom
   * takes a centred `vis` window out of it, the placement slides that window, and the
   * result is stretched to the canvas. Every step here is copied from `add_background`
   * on purpose: the preview is only useful if it fails wherever the engine fails.
   */
  const backgroundWindow = createMemo(() => {
    const size = backdropSize();
    const canvas = canvasSize();
    if (!size || !(size.w > 0) || !(size.h > 0)) return null;
    return backgroundCrop(
      size.w,
      size.h,
      canvas.w,
      canvas.h,
      clamp(props.imageZoom, 1, 3),
      finite(props.imageX, 0),
      finite(props.imageY, 0),
    );
  });

  /**
   * Where the background `<img>` sits, in display pixels.
   *
   * The engine stretches the `vis` crop to the canvas, so one source pixel of the
   * picture is `s = canvasDisplayWidth / visW` display pixels wide. The whole `base`
   * window is drawn at that scale and the crop offset is subtracted, then the canvas
   * clips it: the visible region is exactly the engine's crop, at the engine's scale.
   */
  const fillGeometry = createMemo(() => {
    const win = backgroundWindow();
    const display = layout();
    const size = backdropSize();
    if (!win || !size || !(display.canvasW > 0) || !(win.visW > 0)) {
      // Without the picture's own size only a cover fit is honest; the file is still
      // composed by the engine, so the dialog must not stall on this.
      return { width: '100%', height: '100%', left: '0', top: '0' };
    }
    // One source pixel is `s` display pixels wide: the engine stretches the visible
    // window to the canvas. Draw the whole picture at that scale and subtract where
    // the cover window and the pan start in the source. Forcing the source into the
    // base window's box instead (the old code) stretched non-square pictures and
    // showed a different region than the file.
    const s = display.canvasW / win.visW;
    return {
      width: `${(size.w * s).toFixed(2)}px`,
      height: `${(size.h * s).toFixed(2)}px`,
      left: `${(-(win.baseX + win.cropX) * s).toFixed(2)}px`,
      top: `${(-(win.baseY + win.cropY) * s).toFixed(2)}px`,
    };
  });

  /**
   * The blur in display pixels: the engine's sigma lives in the picture's own pixels,
   * and `s` is how far one of those is stretched here. Capped so no slider position
   * makes the compositor crawl.
   */
  const fillBlur = createMemo(() => {
    const source = clamp(props.bgBlur, 0, 50);
    const win = backgroundWindow();
    const display = layout();
    const scale = win && win.visW > 0 && display.canvasW > 0 ? display.canvasW / win.visW : 1;
    return Math.min(MAX_BLUR_PX, source * scale);
  });

  /**
   * One drag handler owns the canvas.
   *
   * Every child of the canvas refuses the pointer (`pointer-events: none` in CSS), the
   * press is captured, and the move/up listeners live exactly as long as the gesture:
   * a listener that leaked past `pointerup` kept moving whichever slider the pointer
   * drifted onto, with the cursor stuck in the drag state.
   */
  function startDrag(event: PointerEvent & { currentTarget: HTMLDivElement }): void {
    if (props.mode !== 'image' || !backdrop() || !props.onPlace) return;
    event.preventDefault();
    const target = event.currentTarget;
    const rect = target.getBoundingClientRect();
    const width = Math.max(1, rect.width);
    const height = Math.max(1, rect.height);
    const start = {
      x: event.clientX,
      y: event.clientY,
      bx: finite(props.imageX, 0),
      by: finite(props.imageY, 0),
    };
    try {
      target.setPointerCapture(event.pointerId);
    } catch {
      // The pointer was already released; the listeners below still clean up.
    }
    const move = (e: PointerEvent): void => {
      // A full canvas width drag walks the whole free range, which is 200 units of
      // `bg_x`. The picture follows the pointer: pulling right shows what was left of
      // it, so the value grows instead of fighting the hand.
      const dx = ((e.clientX - start.x) / width) * 200;
      const dy = ((e.clientY - start.y) / height) * 200;
      // The field is a 1-step percentage in -100..100.
      props.onPlace?.(
        Math.min(100, Math.max(-100, Math.round(start.bx + dx))),
        Math.min(100, Math.max(-100, Math.round(start.by + dy))),
      );
    };
    const stop = (): void => {
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', stop);
      target.removeEventListener('pointercancel', stop);
      target.removeEventListener('lostpointercapture', stop);
      try {
        if (target.hasPointerCapture(event.pointerId)) target.releasePointerCapture(event.pointerId);
      } catch {
        // Already released by the platform.
      }
    };
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', stop);
    target.addEventListener('pointercancel', stop);
    target.addEventListener('lostpointercapture', stop);
  }

  return (
    <div class="photo-preview">
      <div class="background-stage">
        <div
          class="background-canvas"
          classList={{
            'can-move': props.mode === 'image' && Boolean(backdrop()) && Boolean(props.onPlace),
          }}
          onPointerDown={startDrag}
          style={{
            width: `${layout().canvasW}px`,
            height: `${layout().canvasH}px`,
            // Only the fill lives on the canvas. The picture is an <img>, so the
            // shorthand cannot compete with it.
            background: fill(),
          }}
        >
          <Show when={props.mode === 'image' && backdrop()}>
            {/* CSS blur fades to transparent past the element's own edge. An
                unblurred copy underneath used to fill that bleed — which painted the
                sharp picture along the canvas border (the "left part of the
                background" that stayed readable). A slightly enlarged copy with the
                same blur keeps the border opaque without ever showing sharp pixels:
                the fade happens outside the canvas. */}
            <img
              class="background-image background-bleed"
              src={backdrop()}
              alt=""
              aria-hidden="true"
              draggable={false}
              style={{
                ...fillGeometry(),
                filter: fillBlur() > 0 ? `blur(${fillBlur().toFixed(2)}px)` : 'none',
                transform: fillBlur() > 0 ? 'scale(1.25)' : 'none',
              }}
            />
            <img
              class="background-image background-fill-image"
              src={backdrop()}
              alt=""
              aria-hidden="true"
              draggable={false}
              style={{
                ...fillGeometry(),
                // One property changes as the slider moves: no covering layer is ever
                // inserted or removed, which is what froze the dialog.
                filter: fillBlur() > 0 ? `blur(${fillBlur().toFixed(2)}px)` : 'none',
              }}
            />
          </Show>
          <Show when={src()}>
            <img
              class="background-photo"
              src={src()}
              alt=""
              draggable={false}
              style={{
                width: `${layout().photoW}px`,
                height: `${layout().photoH}px`,
                'border-radius': `${layout().radiusPx}px`,
                'box-shadow':
                  layout().shadowPx > 0
                    ? `${(Math.sin(layout().shadowAngle) * layout().shadowPx * 1.0).toFixed(1)}px ${(-Math.cos(layout().shadowAngle) * layout().shadowPx * 1.0).toFixed(1)}px ${(layout().shadowPx * 2).toFixed(1)}px rgba(0, 0, 0, ${layout().shadowAlpha.toFixed(2)})`
                    : 'none',
              }}
              onLoad={(event) => {
                const image = event.currentTarget;
                if (image.naturalWidth > 0) {
                  setNatural({ w: image.naturalWidth, h: image.naturalHeight });
                }
              }}
            />
          </Show>
        </div>
        {/* Overlaid, never in flow: a line of status text must not push the controls. */}
        <Show when={!src()}>
          <p class="crop-placeholder background-note">{note() || 'Loading…'}</p>
        </Show>
      </div>
      <p class="preview-caption">
        Canvas <strong>{Math.round(canvasSize().w)} × {Math.round(canvasSize().h)}</strong>
        <span class="preview-caption-note">
          the frame that will be saved; the grey around it is this window
        </span>
      </p>
    </div>
  );
}
