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
 *
 * The frame is always mounted and the render state is a badge *over* it: a transient
 * "Rendering…" line in the flow would change the band's height and shift every field
 * below it on every keystroke. The previous image stays visible until its replacement
 * arrives, and a response from an older request is dropped by token.
 */
export default function PhotoPreview(props: PhotoPreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('');
  const [rendering, setRendering] = createSignal(false);
  let timer: number | undefined;
  let token = 0;

  createEffect(() => {
    const path = props.path;
    const request = JSON.stringify(props.params);
    window.clearTimeout(timer);
    // Invalidate whatever is in flight the moment the parameters change.
    const mine = ++token;
    if (!path) {
      setSrc('');
      setRendering(false);
      setNote('Preview unavailable; the settings still apply to the saved file.');
      return;
    }
    timer = window.setTimeout(() => {
      setRendering(true);
      void ipc
        .photoPreview(path, JSON.parse(request) as Record<string, unknown>)
        .then((produced) => {
          if (mine !== token) return;
          setSrc(ipc.assetUrl(produced));
          setNote('');
          setRendering(false);
        })
        .catch((error) => {
          if (mine !== token) return;
          setNote(error instanceof Error ? error.message : 'Preview unavailable.');
          setRendering(false);
        });
    }, 250);
  });

  onCleanup(() => {
    window.clearTimeout(timer);
    token += 1;
  });

  return (
    <div class="photo-preview">
      <div class="photo-frame">
        <Show when={src()} fallback={<p class="crop-placeholder">{note() || 'Loading…'}</p>}>
          <img src={src()} alt="Edited preview" draggable={false} />
        </Show>
        <Show when={rendering() || (note() && src())}>
          <div class="preview-status" role="status" aria-live="polite">
            {rendering() ? 'Rendering…' : note()}
          </div>
        </Show>
      </div>
    </div>
  );
}
