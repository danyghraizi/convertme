import { For, Show, createEffect, createMemo, createSignal, onCleanup } from 'solid-js';
import { Portal } from 'solid-js/web';
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

/** The landing position, drawn in the grid while a card is lifted. */
interface SlotCell {
  slot: true;
  width: number;
  height: number;
}

type Cell = Entry | SlotCell;

interface DragState {
  entry: Entry;
  fromIndex: number;
  pointerId: number;
  x: number;
  y: number;
  slot: SlotCell;
}

interface PendingPress {
  pointerId: number;
  startX: number;
  startY: number;
  entry: Entry;
  index: number;
}

function isSlot(cell: Cell): cell is SlotCell {
  return 'slot' in cell;
}

/**
 * The page organizer: one card per output page, dragged into order.
 *
 * Each card keeps an `id` fixed at load, so a move, a duplicate or a removal never
 * renumbers the card the pointer is on (a list that re-sorts itself under a drag is
 * how the row you grabbed becomes a different row). The value is the sequence of
 * source pages plus one rotation per output position — the same shape the engine
 * reads as `order` and `rotate`.
 *
 * Dragging is pointer-driven, not HTML drag-and-drop: the native drag image is a
 * full-size translucent copy of the card that covers the grid and says nothing about
 * the landing position. Here the card is lifted into a small ghost, the source leaves
 * the flow, a dashed slot opens where it will land, and the head states the target.
 */
export default function PageOrganizer(props: PageOrganizerProps) {
  const [seed, setSeed] = createSignal<Entry[]>([]);
  const [entries, setEntries] = createSignal<Entry[]>([]);
  const [note, setNote] = createSignal('');
  const [status, setStatus] = createSignal('');
  const [selectedId, setSelectedId] = createSignal<number | null>(null);
  const [drag, setDrag] = createSignal<DragState | null>(null);
  const [target, setTarget] = createSignal<number | null>(null);
  let nextId = 1;
  let loadToken = 0;
  let gridEl: HTMLOListElement | undefined;
  let pending: PendingPress | null = null;

  // The cards in the order they are drawn. While a card is lifted it is replaced by
  // the slot, so every child's index is also its final output position.
  const display = createMemo<Cell[]>(() => {
    const lifted = drag();
    if (!lifted) return entries();
    const rest = entries().filter((entry) => entry.id !== lifted.entry.id);
    const at = Math.max(0, Math.min(target() ?? 0, rest.length));
    return [...rest.slice(0, at), lifted.slot, ...rest.slice(at)];
  });

  // Every file has its own pages. Loading only on mount left the second file of a
  // multi-file dialog showing the first file's count and cards. Key the load on the
  // path, and ignore a response that lands after the path changed.
  createEffect(() => {
    const path = props.path;
    const token = ++loadToken;
    cancelPointer();
    setDrag(null);
    setTarget(null);
    setEntries([]);
    setSeed([]);
    setNote('');
    setStatus('');
    setSelectedId(null);
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
    setSelectedId(copy.id);
    setStatus(`Page ${copy.page} duplicated at position ${index + 2} of ${list.length + 1}.`);
  }

  function rotate(index: number): void {
    const entry = entries()[index];
    publish(
      entries().map((candidate, position) =>
        position === index ? { ...candidate, rotate: (candidate.rotate + 90) % 360 } : candidate,
      ),
    );
    setSelectedId(entry.id);
    setStatus(`Page ${entry.page} rotated to ${(entry.rotate + 90) % 360}°.`);
  }

  function remove(index: number): void {
    const list = entries();
    if (list.length <= 1) {
      setNote('A document needs at least one page.');
      return;
    }
    const next = list.filter((_, position) => position !== index);
    publish(next);
    const neighbour = next[Math.min(index, next.length - 1)];
    setSelectedId(neighbour?.id ?? null);
    setStatus(`Page ${list[index].page} removed; ${next.length} pages remain.`);
  }

  function reset(): void {
    setNote('');
    setStatus('Order reset to the document as it was.');
    setSelectedId(null);
    publish(seed());
  }

  function number(entry: Entry): string {
    return `${entry.page}`;
  }

  /**
   * The insertion index among the cards that remain. A direct hit on a card splits
   * at its horizontal middle; a pointer in a gap belongs to the nearest card, before
   * or after it in reading order.
   */
  function insertionIndex(x: number, y: number): number {
    if (!gridEl) return target() ?? 0;
    const cards = Array.from(gridEl.querySelectorAll<HTMLElement>('[data-page-id]'));
    if (cards.length === 0) return 0;
    const rects = cards.map((card) => card.getBoundingClientRect());
    for (let index = 0; index < rects.length; index += 1) {
      const rect = rects[index];
      if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
        return x < rect.left + rect.width / 2 ? index : index + 1;
      }
    }
    let nearest = 0;
    let distance = Infinity;
    for (let index = 0; index < rects.length; index += 1) {
      const rect = rects[index];
      const dx = x - (rect.left + rect.width / 2);
      const dy = y - (rect.top + rect.height / 2);
      const next = dx * dx + dy * dy;
      if (next < distance) {
        distance = next;
        nearest = index;
      }
    }
    const rect = rects[nearest];
    if (y < rect.top) return nearest;
    if (y > rect.bottom) return nearest + 1;
    return x < rect.left + rect.width / 2 ? nearest : nearest + 1;
  }

  function beginPointer(event: PointerEvent, entry: Entry, index: number): void {
    // WebKitGTK reports a left mouse press as button 0; the crop/annotation
    // surfaces do not gate on `isPrimary` either. Only secondary buttons are ignored.
    if (pending || event.button > 0) return;
    const origin = event.target;
    if (origin instanceof Element && origin.closest('button')) return;
    pending = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      entry,
      index,
    };
    setNote('');
    window.addEventListener('pointermove', onPointerMove, { passive: false });
    window.addEventListener('pointerup', onPointerUp);
    window.addEventListener('pointercancel', onPointerUp);
  }

  function startDrag(event: PointerEvent): void {
    if (!pending) return;
    const slot: SlotCell = {
      slot: true,
      width: pending.entry.width,
      height: pending.entry.height,
    };
    setDrag({
      entry: pending.entry,
      fromIndex: pending.index,
      pointerId: pending.pointerId,
      x: event.clientX,
      y: event.clientY,
      slot,
    });
    setTarget(pending.index);
    setSelectedId(pending.entry.id);
    setStatus('');
    // Keep pointerup and pointermove even when the gesture leaves the grid.
    try {
      gridEl?.setPointerCapture(event.pointerId);
    } catch {
      // WebKit throws when the pointer is already gone; the window listeners still
      // end the gesture on the next pointerup.
    }
  }

  function onPointerMove(event: PointerEvent): void {
    if (!pending || event.pointerId !== pending.pointerId) return;
    const lifted = drag();
    if (!lifted) {
      const dx = event.clientX - pending.startX;
      const dy = event.clientY - pending.startY;
      if (Math.hypot(dx, dy) < 5) return;
      startDrag(event);
    }
    event.preventDefault();
    setDrag((current) =>
      current ? { ...current, x: event.clientX, y: event.clientY } : current,
    );
    maybeScroll(event.clientY);
    const at = insertionIndex(event.clientX, event.clientY);
    if (at !== target()) setTarget(at);
  }

  function onPointerUp(event: PointerEvent): void {
    if (!pending || event.pointerId !== pending.pointerId) return;
    const lifted = drag();
    if (lifted) {
      const rest = entries().filter((entry) => entry.id !== lifted.entry.id);
      const at = Math.max(0, Math.min(target() ?? lifted.fromIndex, rest.length));
      const next = [...rest.slice(0, at), lifted.entry, ...rest.slice(at)];
      const changed = next.some((entry, index) => entry.id !== entries()[index]?.id);
      if (changed) publish(next);
      setSelectedId(lifted.entry.id);
      setStatus(
        `Page ${lifted.entry.page} ${changed ? 'moved' : 'stays'} at position ${at + 1} of ${next.length}.`,
      );
    } else if (pending.entry.id) {
      setSelectedId(pending.entry.id);
      setStatus(`Page ${pending.entry.page} selected.`);
    }
    cancelPointer();
    setDrag(null);
    setTarget(null);
  }

  function cancelPointer(): void {
    if (pending) {
      try {
        if (gridEl?.hasPointerCapture(pending.pointerId)) {
          gridEl.releasePointerCapture(pending.pointerId);
        }
      } catch {
        // The pointer already ended.
      }
    }
    pending = null;
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
    window.removeEventListener('pointercancel', onPointerUp);
  }

  /** Keep the grabbed card reachable in a long grid. */
  function maybeScroll(y: number): void {
    if (!gridEl) return;
    const rect = gridEl.getBoundingClientRect();
    if (y < rect.top + 28) gridEl.scrollBy(0, -12);
    else if (y > rect.bottom - 28) gridEl.scrollBy(0, 12);
  }

  function handleKey(event: KeyboardEvent, entry: Entry, index: number): void {
    const origin = event.target;
    if (origin instanceof Element && origin.closest('button')) return;
    const delta = event.key === 'ArrowLeft' ? -1 : event.key === 'ArrowRight' ? 1 : 0;
    if (delta === 0) return;
    const to = index + delta;
    if (to < 0 || to >= entries().length) return;
    event.preventDefault();
    move(index, to);
    setSelectedId(entry.id);
    setStatus(`Page ${entry.page} moved to position ${to + 1} of ${entries().length}.`);
  }

  /**
   * The lifted card is a small thumbnail beside the pointer, never a full card over
   * the grid: the landing slot and the pages around it must stay visible.
   */
  function ghostStyle(lifted: DragState): Record<string, string> {
    const width = Math.min(104, Math.round(window.innerWidth * 0.2));
    const height = Math.min(220, width * (lifted.entry.height / Math.max(1, lifted.entry.width)) + 16);
    const left = Math.max(8, Math.min(lifted.x + 16, window.innerWidth - width - 8));
    const top = Math.max(8, Math.min(lifted.y + 16, window.innerHeight - height - 8));
    return { left: `${left}px`, top: `${top}px`, width: `${width}px` };
  }

  function headStatus(): string {
    const lifted = drag();
    if (lifted) {
      return `Page ${lifted.entry.page} → position ${(target() ?? 0) + 1} of ${entries().length}`;
    }
    return status();
  }

  onCleanup(cancelPointer);

  return (
    <div class="page-organizer" classList={{ 'is-dragging': drag() !== null }}>
      <div class="page-organizer-head">
        <span class="page-count">{entries().length} pages</span>
        <Show when={headStatus()}>
          <span class="page-drag-status" role="status" aria-live="polite">
            {headStatus()}
          </span>
        </Show>
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
      <ol class="page-grid" ref={gridEl} role="list">
        <For each={display()}>
          {(cell, index) => {
            if (isSlot(cell)) {
              return (
                <li
                  class="page-slot"
                  aria-hidden="true"
                  style={{
                    'aspect-ratio': `${Math.max(1, cell.width)} / ${Math.max(1, cell.height)}`,
                  }}
                >
                  <span class="page-slot-label">Drop here</span>
                  <span class="page-slot-position">Position {index() + 1}</span>
                </li>
              );
            }
            return (
              <li
                class="page-card"
                data-page-id={cell.id}
                classList={{ 'is-selected': selectedId() === cell.id }}
                tabIndex={0}
                aria-current={selectedId() === cell.id ? 'true' : 'false'}
                aria-label={`Page ${cell.page}, position ${index() + 1} of ${display().length}${
                  selectedId() === cell.id ? ', selected' : ''
                }`}
                title="Drag to reorder; the arrow keys also move the card"
                onPointerDown={(event) => beginPointer(event, cell, index())}
                onKeyDown={(event) => handleKey(event, cell, index())}
              >
                <div
                  class="page-face"
                  style={{
                    'aspect-ratio': `${Math.max(1, cell.width)} / ${Math.max(1, cell.height)}`,
                    transform: cell.rotate === 0 ? undefined : `rotate(${cell.rotate}deg)`,
                  }}
                >
                  <Show
                    when={cell.thumb}
                    fallback={<span class="page-number-only">{number(cell)}</span>}
                  >
                    {(thumb) => <img src={thumb()} alt={`Page ${cell.page}`} draggable={false} />}
                  </Show>
                </div>
                <span class="page-badge" title="Output position">
                  {index() + 1}
                </span>
                <div class="page-foot">
                  <span class="page-meta">
                    Page {cell.page}
                    <Show when={cell.rotate !== 0}>
                      <span class="page-rotation"> · {cell.rotate}°</span>
                    </Show>
                  </span>
                  <div class="page-tools">
                    <button
                      type="button"
                      class="page-tool"
                      aria-label={`Duplicate page ${cell.page}`}
                      title="Duplicate"
                      onPointerDown={(event) => event.stopPropagation()}
                      onClick={() => duplicate(index())}
                    >
                      ⧉
                    </button>
                    <button
                      type="button"
                      class="page-tool"
                      aria-label={`Rotate page ${cell.page}`}
                      title="Rotate 90°"
                      onPointerDown={(event) => event.stopPropagation()}
                      onClick={() => rotate(index())}
                    >
                      ↻
                    </button>
                    <button
                      type="button"
                      class="page-tool"
                      aria-label={`Remove page ${cell.page}`}
                      title="Remove"
                      onPointerDown={(event) => event.stopPropagation()}
                      onClick={() => remove(index())}
                    >
                      ×
                    </button>
                  </div>
                </div>
              </li>
            );
          }}
        </For>
      </ol>
      <p class="field-help page-hint">
        Drag a card to move it — the dashed slot is where it will land. Click a card to
        select it; the Left/Right arrow keys also move it.
      </p>
      <Show when={drag()}>
        {(lifted) => (
          <Portal>
            <div
              class="page-ghost"
              aria-hidden="true"
              style={ghostStyle(lifted())}
            >
              <div
                class="page-ghost-face"
                style={{
                  'aspect-ratio': `${Math.max(1, lifted().entry.width)} / ${Math.max(1, lifted().entry.height)}`,
                }}
              >
                <Show
                  when={lifted().entry.thumb}
                  fallback={
                    <span class="page-number-only">{number(lifted().entry)}</span>
                  }
                >
                  {(thumb) => (
                    <img src={thumb()} alt="" draggable={false} />
                  )}
                </Show>
              </div>
              <span class="page-ghost-page">Page {lifted().entry.page}</span>
              <span class="page-ghost-target">→ {(target() ?? 0) + 1}</span>
            </div>
          </Portal>
        )}
      </Show>
    </div>
  );
}
