import { Show, createEffect, createSignal, onCleanup } from 'solid-js';
import * as ipc from '../ipc';
import './collage-preview.css';

interface CollagePreviewProps {
  paths: string[];
  /** The flattened parameters the job will receive. */
  params: Record<string, unknown>;
}

/**
 * The composition as the engine will write it. The render is debounced, and a stale
 * response is dropped rather than paired with newer settings (a preview that lags one
 * drag behind is worse than none).
 */
export default function CollagePreview(props: CollagePreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('Loading…');
  let timer: number | undefined;
  let token = 0;

  createEffect(() => {
    const paths = [...props.paths];
    const request = JSON.stringify(props.params);
    window.clearTimeout(timer);
    if (paths.length === 0) {
      setSrc('');
      setNote('Select images to preview the collage.');
      return;
    }
    timer = window.setTimeout(() => {
      const mine = ++token;
      setNote('Rendering…');
      void ipc
        .collagePreview(paths, JSON.parse(request) as Record<string, unknown>)
        .then((path) => {
          if (mine !== token) return;
          setSrc(ipc.assetUrl(path));
          setNote('');
        })
        .catch((error) => {
          if (mine !== token) return;
          setSrc('');
          setNote(error instanceof Error ? error.message : 'Preview unavailable.');
        });
    }, 250);
  });

  onCleanup(() => window.clearTimeout(timer));

  return (
    <div class="collage-preview">
      <Show when={src()} fallback={<p class="crop-placeholder">{note()}</p>}>
        <img src={src()} alt="Collage preview" draggable={false} />
      </Show>
      <Show when={note() && src()}>
        <p class="field-help">{note()}</p>
      </Show>
    </div>
  );
}
