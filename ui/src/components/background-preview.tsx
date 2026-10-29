import { Show, createSignal, onMount } from 'solid-js';
import * as ipc from '../ipc';

/**
 * The composition preview: the photo on its new canvas, with the padding, the corner
 * radius and the fill it will be saved with. It is an approximation built from the
 * same numbers the engine gets — the exact scaling is ImageMagick's.
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
  source: 0,
};

export default function BackgroundPreview(props: BackgroundPreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('');

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

  const ratio = (): number | null => {
    const value = RATIOS[props.aspect];
    return value === undefined || value === 0 ? null : value;
  };

  return (
    <div class="photo-preview">
      <div
        class="background-stage"
        style={{
          'aspect-ratio': ratio() ? String(ratio()) : 'auto',
          background: fill(),
          'padding-top': ratio() ? undefined : `${Math.min(props.padding / 8, 48)}px`,
          'padding-bottom': ratio() ? undefined : `${Math.min(props.padding / 8, 48)}px`,
        }}
      >
        <Show when={src()} fallback={<p class="crop-placeholder">{note() || 'Loading…'}</p>}>
          <img
            src={src()}
            alt=""
            draggable={false}
            style={{
              'border-radius': `${Math.min(props.radius / 6, 24)}px`,
              padding: ratio() ? `${Math.min(props.padding / 8, 48)}px` : '0',
              'box-sizing': 'border-box',
              'max-height': '38vh',
            }}
          />
        </Show>
      </div>
      <p class="field-help">
        A guide to the composition; the saved file uses the exact pixel sizes.
      </p>
    </div>
  );
}
