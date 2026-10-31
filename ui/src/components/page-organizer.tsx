import { For, Show, createEffect, createSignal } from 'solid-js';
import * as ipc from '../ipc';
import type { FieldValue, PagesValue, PdfPage } from '../types';
import './page-organizer.css';

interface PageOrganizerProps {
  path: string | null;
  /** The current `{order, rotate}` — the organizer publishes defaults per file. */
  value: FieldValue;
  onChange: (value: PagesValue) => void;
}

/** One card: the source page, its rotation, and the id that survives every move. */
interface Entry {
  id: number;
  page: number;
  rotate: number;
  thumb: string | null;
  width: number;
  height: number;
}

/**
 * The page organizer: one card per output page, dragged into order.
 *
 * Each card keeps an `id` fixed at load, so a move, a duplicate or a removal never
 * renumbers the card the pointer is on (a list that re-sorts itself under a drag is
 * how the row you grabbed becomes a different row). The value is the sequence of
 * source pages plus one rotation per output position — the same shape the engine
 * reads as `order` and `rotate`.
 */
export default function PageOrganizer(props: PageOrganizerProps) {
  const [seed, setSeed] = createSignal<Entry[]>([]);
  const [entries, setEntries] = createSignal<Entry[]>([]);
  const [note, setNote] = createSignal('');
  const [dragging, setDragging] = createSignal<number | null>(null);
  let nextId = 1;
  let loadToken = 0;

  // Every file has its own pages. Loading only on mount left the second file of a
  // multi-file dialog showing the first file's count and cards. Key the load on the
  // path, and ignore a response that lands after the path changed.
  createEffect(() => {
    const path = props.path;
    const token = ++loadToken;
    setEntries([]);
    setSeed([]);
    setNote('');
    if (!path) {
      setNote('Choose a PDF to organize its pages.');
      return;
    }
    void ipc
      .pdfPages(path)
      .then((pages: PdfPage[]) => {
        if (token !== loadToken) return;
        const loaded: Entry[] = pages.map((page) => ({
          id: nextId++,
          page: page.page,
          rotate: 0,
          thumb: page.thumb ? ipc.assetUrl(page.thumb) : null,
          width: page.width,
          height: page.height,
        }));
        setSeed(loaded);
        setEntries(loaded);
        // Nothing is "done" until the sequence differs from the document as it is.
        props.onChange({ order: loaded.map((entry) => entry.page), rotate: loaded.map(() => 0) });
      })
      .catch((error) => {
        if (token !== loadToken) return;
        setNote(error instanceof Error ? error.message : 'Pages unavailable');
      });
  });

  function publish(next: Entry[]): void {
    setEntries(next);
    props.onChange({
      order: next.map((entry) => entry.page),
      rotate: next.map((entry) => entry.rotate),
    });
  }

  function move(from: number, to: number): void {
    const list = entries();
    if (to < 0 || to >= list.length || from === to) return;
    const next = [...list];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    publish(next);
  }

  function duplicate(index: number): void {
    const list = entries();
    const copy = { ...list[index], id: nextId++ };
    publish([...list.slice(0, index + 1), copy, ...list.slice(index + 1)]);
  }

  function rotate(index: number): void {
    publish(
      entries().map((entry, position) =>
        position === index ? { ...entry, rotate: (entry.rotate + 90) % 360 } : entry,
      ),
    );
  }

  function remove(index: number): void {
    const list = entries();
    if (list.length <= 1) {
      setNote('A document needs at least one page.');
      return;
    }
    publish(list.filter((_, position) => position !== index));
  }

  function reset(): void {
    setNote('');
    publish(seed());
  }

  function number(entry: Entry): string {
    return `${entry.page}`;
  }

  return (
    <div class="page-organizer">
      <div class="page-organizer-head">
        <span class="page-count">{entries().length} pages</span>
        <button
          type="button"
          class="page-reset"
          aria-label="Reset Order"
          onClick={reset}
          disabled={entries().length === 0}
        >
          Reset Order
        </button>
      </div>
      <Show when={note()}>
        <p class="field-help" role="status">
          {note()}
        </p>
      </Show>
      <ol class="page-grid" role="list">
        <For each={entries()}>
          {(entry, index) => (
            <li
              class="page-card"
              classList={{ 'is-dragging': dragging() === entry.id }}
              draggable={true}
              onDragStart={(event) => {
                setDragging(entry.id);
                event.dataTransfer?.setData('text/plain', String(index()));
              }}
              onDragOver={(event) => {
                event.preventDefault();
                const from = entries().findIndex((candidate) => candidate.id === dragging());
                if (from >= 0) move(from, index());
              }}
              onDragEnd={() => setDragging(null)}
            >
              <div
                class="page-face"
                style={{
                  'aspect-ratio': `${Math.max(1, entry.width)} / ${Math.max(1, entry.height)}`,
                  transform: entry.rotate === 0 ? undefined : `rotate(${entry.rotate}deg)`,
                }}
              >
                <Show
                  when={entry.thumb}
                  fallback={<span class="page-number-only">{number(entry)}</span>}
                >
                  {(thumb) => <img src={thumb()} alt={`Page ${entry.page}`} draggable={false} />}
                </Show>
              </div>
              <span class="page-badge">{index() + 1}</span>
              <div class="page-tools">
                <button
                  type="button"
                  class="page-tool"
                  aria-label={`Duplicate page ${entry.page}`}
                  title="Duplicate"
                  onClick={() => duplicate(index())}
                >
                  ⧉
                </button>
                <button
                  type="button"
                  class="page-tool"
                  aria-label={`Rotate page ${entry.page}`}
                  title="Rotate 90°"
                  onClick={() => rotate(index())}
                >
                  ↻
                </button>
                <button
                  type="button"
                  class="page-tool"
                  aria-label={`Remove page ${entry.page}`}
                  title="Remove"
                  onClick={() => remove(index())}
                >
                  ×
                </button>
              </div>
              <span class="page-meta">
                Page {entry.page}
                <Show when={entry.rotate !== 0}>
                  <span class="page-rotation"> · {entry.rotate}°</span>
                </Show>
              </span>
            </li>
          )}
        </For>
      </ol>
    </div>
  );
}
