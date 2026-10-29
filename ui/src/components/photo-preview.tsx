import { Show, createEffect, createSignal } from 'solid-js';
import * as ipc from '../ipc';

/**
 * A live approximation of the adjustment sliders. CSS filters cover exposure,
 * brightness, contrast, saturation and hue; temperature is a tinted overlay. Gamma has
 * no CSS equivalent, so it is applied to the saved file only and the caption says so.
 */
interface PhotoPreviewProps {
  path: string | null;
  exposure: number;
  brightness: number;
  contrast: number;
  saturation: number;
  hue: number;
  temperature: number;
}

export default function PhotoPreview(props: PhotoPreviewProps) {
  const [src, setSrc] = createSignal('');
  const [note, setNote] = createSignal('');

  // Follows the file the dialog is previewing.
  createEffect(() => {
    const path = props.path;
    if (!path) {
      setNote('Preview unavailable; the settings still apply to the saved file.');
      return;
    }
    setNote('');
    void ipc
      .previewThumb(path, 1024)
      .then((url) => setSrc(ipc.assetUrl(url)))
      .catch(() => setNote('Preview unavailable.'));
  });

  const filter = (): string => {
    const brightness = Math.max(0.05, 1 + props.brightness / 100) * 2 ** props.exposure;
    const contrast = Math.max(0.05, 1 + props.contrast / 100);
    const saturate = Math.max(0, 1 + props.saturation / 100);
    const hue = props.hue;
    return `brightness(${brightness.toFixed(3)}) contrast(${contrast.toFixed(3)}) saturate(${saturate.toFixed(3)}) hue-rotate(${hue.toFixed(1)}deg)`;
  };

  const tint = (): string => {
    const warm = Math.max(-1, Math.min(1, props.temperature / 100));
    if (warm === 0) return 'transparent';
    return warm > 0
      ? `rgba(255, 168, 82, ${(warm * 0.35).toFixed(3)})`
      : `rgba(82, 148, 255, ${(-warm * 0.35).toFixed(3)})`;
  };

  return (
    <div class="photo-preview">
      <Show when={src()} fallback={<p class="crop-placeholder">{note() || 'Loading…'}</p>}>
        <div class="photo-frame">
          <img src={src()} alt="" style={{ filter: filter() }} draggable={false} />
          <span class="photo-tint" style={{ background: tint() }} />
        </div>
      </Show>
      <p class="field-help">Gamma is applied to the saved file; the preview approximates it.</p>
    </div>
  );
}
