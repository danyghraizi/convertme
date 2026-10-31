import { For, Show, createEffect, createSignal } from 'solid-js';
import * as ipc from '../ipc';
import { baseName } from '../format';
import { asOrder } from '../fields';
import type { FieldValue } from '../types';
import './file-order.css';

interface FileOrderProps {
  /** The selection, in arrival order. Row identity is the index into this list. */
  files: { path: string; label: string }[];
  /** Indices into `files`; empty means "leave the selection order alone". */
  value: FieldValue;
  onChange: (value: number[]) => void;
}

/**
 * Reorder a multi-file tool's inputs (merge, join, collage).
 *
 * The value is a list of indices into the original selection, so moving a row never
 * renumbers anything: a thumbnail, its name and its place in the job stay attached
 * to the same file however often the list is sorted.
 */
export default function FileOrder(props: FileOrderProps) {
  const [thumbs, setThumbs] = createSignal<Record<string, string>>({});

  const order = (): number[] => {
    const current = asOrder(props.value);
    // An untouched list has no order to report: it is the selection order, and the
    // engine applies exactly that when it sees nothing.
    if (current.length === props.files.length) return current;
    return props.files.map((_, index) => index);
  };

  // One thumbnail request per file, not per render: the list re-sorts on every move.
  createEffect(() => {
    for (const file of props.files) {
      if (thumbs()[file.path]) continue;
      void ipc
        .previewThumb(file.path, 96)
        .then((url) => setThumbs((current) => ({ ...current, [file.path]: ipc.assetUrl(url) })))
        .catch(() => setThumbs((current) => ({ ...current, [file.path]: '' })));
    }
  });

  function move(position: number, delta: number): void {
    const next = [...order()];
    const target = position + delta;
    if (target < 0 || target >= next.length) return;
    [next[position], next[target]] = [next[target], next[position]];
    props.onChange(next);
  }

  return (
    <div class="file-order">
      <ol class="file-order-list">
        <For each={order()}>
          {(index, position) => {
            const file = (): { path: string; label: string } =>
              props.files[index] ?? { path: '', label: '' };
            return (
              <li class="file-order-row">
                <span class="file-order-number">{position() + 1}</span>
                <span class="file-order-thumb">
                  <Show when={thumbs()[file().path]} fallback={<span class="file-order-blank" />}>
                    {(src) => <img src={src()} alt="" draggable={false} />}
                  </Show>
                </span>
                <span class="file-order-name" title={file().path}>
                  {file().label || baseName(file().path)}
                </span>
                <button
                  type="button"
                  class="file-order-step"
                  aria-label={`Move ${file().label || baseName(file().path)} up`}
                  disabled={position() === 0}
                  onClick={() => move(position(), -1)}
                >
                  ↑
                </button>
                <button
                  type="button"
                  class="file-order-step"
                  aria-label={`Move ${file().label || baseName(file().path)} down`}
                  disabled={position() === order().length - 1}
                  onClick={() => move(position(), 1)}
                >
                  ↓
                </button>
              </li>
            );
          }}
        </For>
      </ol>
      <Show when={order().length === 0}>
        <p class="field-help">Select two or more files to arrange them.</p>
      </Show>
    </div>
  );
}
