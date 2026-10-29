import { Show, createMemo, createSignal, onMount } from 'solid-js';
import * as ipc from '../ipc';

/**
 * The composition preview: the photo on its new canvas, with the padding, the corner
 * radius, the ratio and the fill it will be saved with. Every size is computed from the
 * numbers the engine gets, then scaled once to fit the panel, so the preview and the
 * file agree on proportions.
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

export default function BackgroundPreview(props: BackgroundPreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('');
  const [natural, setNatural] = createSignal({ w: 4, h: 3 });
  const [real, setReal] = createSignal<{ w: number; h: number } | null>(null);
  const [backdrop, setBackdrop] = createSignal('');

  onMount(() => {
    const path = props.path;
    if (!path) {
      setNote('Preview unavailable; the settings still apply to the saved file.');
      return;
    }
    void ipc
      .previewThumb(path, 1024)
      .then((url) => setSrc(ipc.assetUrl(url)))
      .catch(() => setNote('Preview unavailable.'));
    // The drawn bitmap is a thumbnail; the numbers must be the file's own pixels.
    void ipc.imageSize(path).then((size) => {
      if (size) setReal(size);
    });
  });

  // The chosen background image is shown too: choosing it and seeing nothing was the
  // whole complaint about that mode.
  const chosenImage = createMemo(() => props.image?.trim() ?? '');
  createMemo(() => {
    const path = chosenImage();
    if (!path) {
      setBackdrop('');
      return;
    }
    setBackdrop(ipc.assetUrl(path));
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
    return props.mode === 'colour' ? props.color || '#101418' : 'transparent';
  };

  /** The canvas in source pixels: what the file will be. */
  const canvasSize = createMemo(() => {
    const source = real() ?? natural();
    const { w, h } = source;
    const pad = Math.round((Math.max(0, props.paddingPct) / 100) * Math.min(w, h));
    let canvasW = w + 2 * pad;
    let canvasH = h + 2 * pad;
    const ratio = RATIOS[props.aspect];
    if (ratio) {
      if (canvasW / canvasH > ratio) canvasH = canvasW / ratio;
      else canvasW = canvasH * ratio;
    }
    return { w: canvasW, h: canvasH };
  });

  /** Canvas and photo sizes in source pixels, then one scale to fit the panel. */
  const layout = createMemo(() => {
    // Draw with the thumbnail's shape, but place the photo by the real proportions.
    const realSize = real();
    const { w, h } = realSize ?? natural();
    const pad = (Math.max(0, props.paddingPct) / 100) * Math.min(w, h);
    const ratio = RATIOS[props.aspect];
    // The canvas always holds the photo plus its padding; a ratio only extends the
    // narrower axis, exactly what the engine does.
    let canvasW = w + 2 * pad;
    let canvasH = h + 2 * pad;
    if (ratio) {
      if (canvasW / canvasH > ratio) canvasH = canvasW / ratio;
      else canvasW = canvasH * ratio;
    }
    const scale = Math.min(MAX_WIDTH / canvasW, MAX_HEIGHT / canvasH);
    const photoW = w * scale;
    const photoH = h * scale;
    return {
      canvasW: canvasW * scale,
      canvasH: canvasH * scale,
      photoW,
      photoH,
      padPx: pad * scale,
      // 100% is a circle, so the slider maps to half the shorter side.
      radiusPx: Math.min(
        (props.radiusPct / 100) * (Math.min(w, h) / 2) * scale,
        Math.min(photoW, photoH) / 2,
      ),
      // Same numbers the engine uses: sigma 0.22 of the shorter side, offset 0.9 sigma,
      // and a CSS blur radius of about twice sigma.
      shadowPx: (props.shadowPct / 100) * Math.min(w, h) * 0.22 * scale,
      shadowAngle: (props.shadowAngle * Math.PI) / 180,
    };
  });

  return (
    <div class="photo-preview">
      <div class="background-stage">
        <Show when={src()} fallback={<p class="crop-placeholder">{note() || 'Loading…'}</p>}>
          <div
            class="background-canvas"
            classList={{ 'can-move': props.mode === 'image' && Boolean(backdrop()) }}
            onPointerDown={(event) => {
              if (props.mode !== 'image' || !backdrop() || !props.onPlace) return;
              const target = event.currentTarget;
              target.setPointerCapture(event.pointerId);
              const start = { x: event.clientX, y: event.clientY, bx: props.imageX, by: props.imageY };
              const rect = target.getBoundingClientRect();
              const move = (e: PointerEvent): void => {
                // The pointer drags the *frame*, so the picture moves the other way: pull
                // right and the picture follows to the right.
                const dx = ((e.clientX - start.x) / Math.max(1, rect.width)) * 200;
                const dy = ((e.clientY - start.y) / Math.max(1, rect.height)) * 200;
                const clamp = (v: number): number => Math.min(100, Math.max(-100, Math.round(v)));
                props.onPlace?.(clamp(start.bx - dx), clamp(start.by - dy));
              };
              const up = (): void => {
                target.removeEventListener('pointermove', move);
                target.removeEventListener('pointerup', up);
                target.removeEventListener('pointercancel', up);
              };
              target.addEventListener('pointermove', move);
              target.addEventListener('pointerup', up);
              target.addEventListener('pointercancel', up);
            }}
            style={{
              width: `${layout().canvasW}px`,
              height: `${layout().canvasH}px`,
              // The fill belongs to the saved canvas, not to this window. Zoom and
              // placement of a chosen picture match the engine's crop window.
              background:
                props.mode === 'image' && backdrop()
                  ? `url(${backdrop()}) center / cover`
                  : fill(),
              'background-size':
                props.mode === 'image' && backdrop()
                  ? `${Math.max(100, props.imageZoom * 100)}% auto`
                  : undefined,
              'background-position':
                props.mode === 'image' && backdrop()
                  ? `calc(50% + ${props.imageX / 2}%) calc(50% + ${props.imageY / 2}%)`
                  : undefined,
            }}
          >
            <img
              src={src()}
              alt=""
              draggable={false}
              style={{
                width: `${layout().photoW}px`,
                height: `${layout().photoH}px`,
                'border-radius': `${layout().radiusPx}px`,
                'box-shadow':
                  layout().shadowPx > 0
                    ? `${(Math.sin(layout().shadowAngle) * layout().shadowPx * 0.9).toFixed(1)}px ${(-Math.cos(layout().shadowAngle) * layout().shadowPx * 0.9).toFixed(1)}px ${(layout().shadowPx * 2).toFixed(1)}px rgba(0, 0, 0, 0.32)`
                    : 'none',
              }}
              onLoad={(event) => {
                const image = event.currentTarget;
                if (image.naturalWidth > 0) {
                  setNatural({ w: image.naturalWidth, h: image.naturalHeight });
                }
              }}
            />
          </div>
        </Show>
      </div>
      <Show when={props.mode === 'image'}>
        <label class="preview-zoom">
          <span>Zoom</span>
          <input
            type="range"
            min="1"
            max="3"
            step="0.05"
            value={props.imageZoom}
            onInput={(event) => props.onZoom?.(Number(event.currentTarget.value))}
          />
          <output>{props.imageZoom.toFixed(1)}×</output>
        </label>
      </Show>
      <p class="preview-caption">
        Canvas <strong>{Math.round(canvasSize().w)} × {Math.round(canvasSize().h)}</strong>
        <span class="preview-caption-note">
          the frame that will be saved; the grey around it is this window
        </span>
      </p>
    </div>
  );
}
