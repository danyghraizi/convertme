import { Show, createEffect, createSignal, onCleanup } from 'solid-js';
import { assetUrl, compressPreview } from '../ipc';
import { formatBytes } from '../format';
import type { CompressPreview as PreviewData } from '../types';

interface CompressPreviewProps {
  /** The file the settings apply to right now (the dialog's shown file). */
  path: string | null;
  /** Exactly the parameters the job will receive. */
  params: Record<string, unknown>;
}

/**
 * The real encoded file, not a drawing of it: the backend encodes the selected file at
 * the dialog's settings and reports the artifact's true size and dimensions. The same
 * request is cached, so submitting the job copies those bytes instead of encoding twice.
 */
export default function CompressPreview(props: CompressPreviewProps) {
  const [data, setData] = createSignal<PreviewData | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal('');
  let seq = 0;
  let lastKey = '';

  createEffect(() => {
    const path = props.path;
    // Read the parameters reactively; the object identity changes on every edit.
    const key = path ? `${path}|${JSON.stringify(props.params)}` : '';
    if (!path) {
      lastKey = '';
      setData(null);
      setError('');
      return;
    }
    if (key === lastKey) return;
    lastKey = key;
    setError('');
    // Encoding is real work: wait for the pointer to settle before spending it.
    const timer = setTimeout(() => {
      const mine = ++seq;
      setBusy(true);
      compressPreview(path, props.params)
        .then((result) => {
          if (mine !== seq) return;
          setData(result);
        })
        .catch((cause) => {
          if (mine !== seq) return;
          setData(null);
          setError(
            cause instanceof Error
              ? cause.message
              : typeof cause === 'string'
                ? cause
                : 'The preview could not be encoded',
          );
        })
        .finally(() => {
          if (mine === seq) setBusy(false);
        });
    }, 400);
    onCleanup(() => clearTimeout(timer));
  });

  /** Percent change against the source; negative means smaller. */
  const saved = (preview: PreviewData): number => {
    if (preview.source_bytes <= 0) return 0;
    return Math.round(((preview.bytes - preview.source_bytes) / preview.source_bytes) * 100);
  };

  return (
    <div class="compress-preview" aria-live="polite">
      <div class="compress-frame">
        <Show
          when={data()}
          fallback={
            <p class="compress-empty">
              {busy() ? 'Encoding the preview…' : (error() || 'No preview yet')}
            </p>
          }
        >
          {(preview) => (
            <img
              class="compress-image"
              src={assetUrl(preview().path)}
              alt="The compressed result"
            />
          )}
        </Show>
        <Show when={busy() && data()}>
          <span class="compress-working" aria-hidden="true" />
        </Show>
      </div>
      <div class="compress-facts">
        <Show when={data()}>
          {(preview) => (
            <>
              <span class="compress-dimensions">
                {preview().width} × {preview().height}
              </span>
              <span class="compress-size">{formatBytes(preview().bytes)}</span>
              <span
                classList={{
                  'compress-delta': true,
                  'compress-saved': saved(preview()) < 0,
                  'compress-grown': saved(preview()) > 0,
                }}
              >
                {saved(preview()) === 0
                  ? 'no change'
                  : `${saved(preview()) < 0 ? '\u2212' : '+'}${Math.abs(saved(preview()))}%`}
              </span>
              <span class="compress-from">from {formatBytes(preview().source_bytes)}</span>
              <span class="compress-note">The encoded file; Run reuses it.</span>
            </>
          )}
        </Show>
      </div>
    </div>
  );
}
