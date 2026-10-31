import { Show, createEffect, createSignal, onCleanup } from 'solid-js';
import * as ipc from '../ipc';

interface PhotoPreviewProps {
  path: string | null;
  /** The flattened parameters the job will receive. */
  params: Record<string, unknown>;
}

/**
 * The photo as the engine will write it, at 1024 px. This is the real filter chain,
 * not a CSS approximation: a preview that reimplements gamma and noise drifts from
 * the saved file within a week.
 */
export default function PhotoPreview(props: PhotoPreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('');
  const [stale, setStale] = createSignal(false);
  let timer: number | undefined;
  let token = 0;

  createEffect(() => {
    const path = props.path;
    const request = JSON.stringify(props.params);
    window.clearTimeout(timer);
    if (!path) {
      setSrc('');
      setNote('Preview unavailable; the settings still apply to the saved file.');
      return;
    }
    timer = window.setTimeout(() => {
      const mine = ++token;
      setStale(true);
      void ipc
        .photoPreview(path, JSON.parse(request) as Record<string, unknown>)
        .then((produced) => {
          if (mine !== token) return;
          setSrc(ipc.assetUrl(produced));
          setNote('');
          setStale(false);
        })
        .catch((error) => {
          if (mine !== token) return;
          setNote(error instanceof Error ? error.message : 'Preview unavailable.');
          setStale(false);
        });
    }, 250);
  });

  onCleanup(() => window.clearTimeout(timer));

  return (
    <div class="photo-preview">
      <Show when={src()} fallback={<p class="crop-placeholder">{note() || 'Loading…'}</p>}>
        <div class="photo-frame">
          <img src={src()} alt="Edited preview" draggable={false} />
        </div>
      </Show>
      <Show when={note() && src()}>
        <p class="field-help">{note()}</p>
      </Show>
      <Show when={stale()}>
        <p class="field-help">Rendering…</p>
      </Show>
    </div>
  );
}
