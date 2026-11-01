import { Show, createEffect, createMemo, createSignal, onCleanup } from 'solid-js';
import { assetUrl, compressPreview, previewThumb } from '../ipc';
import { formatBytes, unreachableTarget } from '../format';
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
 *
 * An encode takes seconds. Until it lands, the source thumbnail stands in — a black
 * frame with a spinner reads as a broken preview, not as work in progress — and a quiet
 * status says so. The numbers below always come from the encoded artifact, never from
 * the stand-in.
 */
export default function CompressPreview(props: CompressPreviewProps) {
  /** The source image, shown the instant the dialog opens or the file switches. */
  const [thumb, setThumb] = createSignal('');
  const [data, setData] = createSignal<PreviewData | null>(null);
  // The fields changed but the shown artifact is still the previous request's. A note
  // computed from the new target and the old bytes would be momentarily wrong.
  const [stale, setStale] = createSignal(false);
  const [error, setError] = createSignal('');
  let seq = 0;
  let thumbSeq = 0;
  let lastKey = '';

  // A new file clears the artifact and shows its own picture at once, with no debounce:
  // the wait is already the encode's, and holding a stale frame makes it feel longer.
  createEffect(() => {
    const path = props.path;
    const mine = ++thumbSeq;
    // A new path abandons any encode of the previous file.
    seq += 1;
    lastKey = '';
    setThumb('');
    setData(null);
    setStale(Boolean(path));
    setError('');
    if (!path) return;
    previewThumb(path, 1024)
      .then((url) => {
        if (mine === thumbSeq) setThumb(assetUrl(url));
      })
      .catch(() => {
        // The artifact still arrives; its status stays until it does.
      });
  });

  createEffect(() => {
    const path = props.path;
    // Read the parameters reactively; the object identity changes on every edit.
    const key = path ? `${path}|${JSON.stringify(props.params)}` : '';
    if (!path) {
      lastKey = '';
      return;
    }
    if (key === lastKey) return;
    lastKey = key;
    const mine = ++seq;
    setStale(true);
    setError('');
    // Encoding is real work: wait for the pointer to settle before spending it.
    const timer = setTimeout(() => {
      compressPreview(path, props.params)
        .then((result) => {
          if (mine !== seq) return;
          setData(result);
          setStale(false);
        })
        .catch((cause) => {
          if (mine !== seq) return;
          setData(null);
          setStale(false);
          setError(
            cause instanceof Error
              ? cause.message
              : typeof cause === 'string'
                ? cause
                : 'The preview could not be encoded',
          );
        });
    }, 400);
    onCleanup(() => clearTimeout(timer));
  });

  /** Percent change against the source; negative means smaller. */
  const saved = (preview: PreviewData): number => {
    if (preview.source_bytes <= 0) return 0;
    return Math.round(((preview.bytes - preview.source_bytes) / preview.source_bytes) * 100);
  };

  const missed = createMemo<number | null>(() => {
    const preview = data();
    if (!preview || stale()) return null;
    return unreachableTarget(preview.bytes, props.params);
  });

  /** The target note keeps its line whether or not it has something to say. */
  const missText = (): string => {
    const target = missed();
    if (target === null) return '';
    return `The target of ${formatBytes(target)} is out of reach; this is the smallest result.`;
  };

  return (
    <div class="compress-preview" aria-live="polite">
      <div class="compress-frame">
        <Show
          when={thumb() || data()}
          fallback={
            <p class="compress-empty">
              {stale() ? 'Encoding the preview…' : error() || 'No preview yet'}
            </p>
          }
        >
          {/* The artifact once encoded; until then the source it was encoded from. */}
          <img
            class="compress-image"
            src={data() ? assetUrl(data()!.path) : thumb()}
            alt={data() ? 'The compressed result' : 'Preview'}
          />
        </Show>
        <Show when={stale() && (thumb() || data())}>
          <span class="compress-working">Encoding the preview…</span>
        </Show>
        <Show when={error() && (thumb() || data())}>
          <span class="compress-error">{error()}</span>
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
      {/* Always present: a note that appears and disappears must not move the fields. */}
      <p class="compress-target-miss">{missText()}</p>
    </div>
  );
}
