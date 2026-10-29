import { Show, createSignal, onMount } from 'solid-js';
import * as ipc from '../ipc';

/**
 * The composition preview: the photo on its new canvas, with the padding, the corner
 * radius and the fill it will be saved with. The numbers are the engine's; only the
 * scale is ours, so dragging a slider shows the same proportion the file will have.
 */
interface BackgroundPreviewProps {
  path: string | null;
  mode: string;
  color: string;
  gradient: string;
  image: string | null;
  padding: number;
  radius: number;
  aspect: string;
}

const RATIOS: Record<string, number> = {
  '1:1': 1,
  '4:3': 4 / 3,
  '16:9': 16 / 9,
  '9:16': 9 / 16,
  '3:4': 3 / 4,
};

const MAX_WIDTH = 430;
const MAX_HEIGHT = 160;

export default function BackgroundPreview(props: BackgroundPreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('');
  const [natural, setNatural] = createSignal({ w: 4, h: 3 });

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
  });

  const fill = (): string => {
    if (props.mode === 'gradient') {
      const [from, to] = (props.gradient || '#f6c26b,#e4572e').split(',');
      return `linear-gradient(160deg, ${from || '#f6c26b'}, ${to || '#e4572e'})`;
    }
    return props.mode === 'colour' ? props.color || '#101418' : 'transparent';
  };

  /** Canvas and photo sizes in source pixels, then one scale to fit the panel. */
  const layout = () => {
    const { w, h } = natural();
    const pad = Math.max(0, props.padding);
    const ratio = RATIOS[props.aspect];
    const canvasW = ratio ? Math.max(w + 2 * pad, (h + 2 * pad) * ratio) : w + 2 * pad;
    const canvasH = ratio ? canvasW / ratio : h + 2 * pad;
    const scale = Math.min(MAX_WIDTH / canvasW, MAX_HEIGHT / canvasH);
    const photoW = w * scale;
    const photoH = h * scale;
    return {
      canvasW: canvasW * scale,
      canvasH: canvasH * scale,
      photoW,
      photoH,
      padPx: pad * scale,
      radiusPx: Math.min(props.radius * scale, Math.min(photoW, photoH) / 2),
    };
  };

  return (
    <div class="photo-preview">
      <div class="background-stage" style={{ background: fill() }}>
        <Show when={src()} fallback={<p class="crop-placeholder">{note() || 'Loading…'}</p>}>
          <div
            class="background-canvas"
            style={{
              width: `${layout().canvasW}px`,
              height: `${layout().canvasH}px`,
              'padding-top': `${layout().padPx}px`,
              'padding-left': `${layout().padPx}px`,
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
      <p class="field-help">
        A guide to the composition; the saved file uses the exact pixel sizes.
      </p>
    </div>
  );
}
