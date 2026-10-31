import { For, Show, createEffect, createSignal, onCleanup } from 'solid-js';
import * as ipc from '../ipc';
import { asShapes } from '../fields';
import type { FieldValue, Shape } from '../types';
import './annotate-editor.css';

type ToolName = 'select' | 'arrow' | 'text' | 'rect' | 'ellipse' | 'pen' | 'number' | 'highlight';

interface AnnotateEditorProps {
  previewPath: string | null;
  value: FieldValue;
  /** The dialog's ink colour: also the default for a new shape. */
  color: string;
  onInk?: (color: string) => void;
  onChange: (value: Shape[]) => void;
}

const TOOLS: { id: ToolName; label: string }[] = [
  { id: 'select', label: 'Select' },
  { id: 'arrow', label: 'Arrow' },
  { id: 'text', label: 'Text' },
  { id: 'rect', label: 'Rectangle' },
  { id: 'ellipse', label: 'Ellipse' },
  { id: 'pen', label: 'Pen' },
  { id: 'number', label: 'Number' },
  { id: 'highlight', label: 'Highlight' },
];

const CLICK_MIN = 0.004;

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

/** Whether a normalised point lies inside a shape's bounding box. */
function contains(shape: Shape, point: { x: number; y: number }): boolean {
  const x2 = shape.x + shape.w;
  const y2 = shape.y + shape.h;
  return (
    point.x >= Math.min(shape.x, x2) &&
    point.x <= Math.max(shape.x, x2) &&
    point.y >= Math.min(shape.y, y2) &&
    point.y <= Math.max(shape.y, y2)
  );
}

export default function AnnotateEditor(props: AnnotateEditorProps) {
  const [preview, setPreview] = createSignal('');
  const [note, setNote] = createSignal('');
  const [tool, setTool] = createSignal<ToolName>('rect');
  const [selected, setSelected] = createSignal<number | null>(null);
  const [draft, setDraft] = createSignal<Shape | null>(null);
  const [live, setLive] = createSignal<Shape[] | null>(null);
  const [text, setText] = createSignal('');
  const [undoCount, setUndoCount] = createSignal(0);

  let surface: HTMLDivElement | undefined;
  let rect: DOMRect | null = null;
  let origin: { x: number; y: number } | null = null;
  let drag: {
    index: number;
    mode: 'move' | 'resize';
    start: { x: number; y: number };
    shape: Shape;
  } | null = null;
  let history: Shape[][] = [];

  const shapes = (): Shape[] => asShapes(props.value);
  const shown = (): Shape[] => live() ?? shapes();

  createEffect(() => {
    const path = props.previewPath;
    if (!path) {
      setNote('Preview unavailable; annotations can still be entered.');
      return;
    }
    void ipc
      .previewThumb(path, 1024)
      .then((url) => setPreview(ipc.assetUrl(url)))
      .catch(() => setNote('Preview unavailable.'));
  });

  onCleanup(() => setPreview(''));

  function push(next: Shape[]): void {
    history = [...history, shapes()].slice(-50);
    setUndoCount(history.length);
    props.onChange(next);
    setLive(null);
  }

  function undo(): void {
    const previous = history.pop();
    setUndoCount(history.length);
    if (!previous) return;
    setSelected(null);
    setLive(null);
    props.onChange(previous);
  }

  function nextNumber(): string {
    let highest = 0;
    for (const shape of shapes()) {
      if (shape.kind !== 'number') continue;
      const value = Number.parseInt(shape.text ?? '', 10);
      if (Number.isFinite(value) && value > highest) highest = value;
    }
    return String(highest + 1);
  }

  function point(event: PointerEvent): { x: number; y: number } {
    const bounds = rect ?? surface?.getBoundingClientRect();
    if (!bounds || bounds.width === 0) return { x: 0, y: 0 };
    return {
      x: clamp01((event.clientX - bounds.left) / bounds.width),
      y: clamp01((event.clientY - bounds.top) / bounds.height),
    };
  }

  function kindFor(name: ToolName): Shape['kind'] {
    switch (name) {
      case 'arrow':
        return 'arrow';
      case 'text':
        return 'text';
      case 'ellipse':
        return 'ellipse';
      case 'pen':
        return 'line';
      case 'number':
        return 'number';
      case 'highlight':
        return 'highlight';
      default:
        return 'rect';
    }
  }

  function begin(event: PointerEvent): void {
    if (!surface || !preview()) return;
    rect = surface.getBoundingClientRect();
    const p = point(event);

    if (tool() === 'select') {
      const list = shapes();
      let hit: number | null = null;
      for (let index = list.length - 1; index >= 0; index -= 1) {
        if (contains(list[index], p)) {
          hit = index;
          break;
        }
      }
      setSelected(hit);
      if (hit !== null) {
        drag = { index: hit, mode: 'move', start: p, shape: { ...list[hit] } };
        setLive(list);
        surface.setPointerCapture(event.pointerId);
      }
      event.preventDefault();
      return;
    }

    if (tool() === 'text') {
      const value = text().trim();
      if (!value) {
        setNote('Type the text in the toolbar, then click the photo.');
        return;
      }
      push([
        ...shapes(),
        {
          kind: 'text',
          x: p.x,
          y: p.y,
          w: 0,
          h: 0,
          text: value,
          color: props.color,
          size: 28,
        },
      ]);
      setNote('');
      return;
    }

    const shape: Shape = {
      kind: kindFor(tool()),
      x: p.x,
      y: p.y,
      w: 0,
      h: 0,
      color: props.color,
    };
    if (tool() === 'number') shape.text = nextNumber();
    if (tool() === 'pen') shape.points = [[p.x, p.y]];
    origin = p;
    setDraft(shape);
    surface.setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function move(event: PointerEvent): void {
    if (drag) {
      const p = point(event);
      const dx = p.x - drag.start.x;
      const dy = p.y - drag.start.y;
      const list = shapes().map((shape, index) => {
        if (index !== drag?.index) return shape;
        if (drag.mode === 'move') {
          const x = Math.min(1 - drag.shape.w, Math.max(0, drag.shape.x + dx));
          const y = Math.min(1 - drag.shape.h, Math.max(0, drag.shape.y + dy));
          return { ...shape, x, y };
        }
        const w = Math.min(1 - drag.shape.x, Math.max(CLICK_MIN, drag.shape.w + dx));
        const h = Math.min(1 - drag.shape.y, Math.max(CLICK_MIN, drag.shape.h + dy));
        return { ...shape, w, h };
      });
      setLive(list);
      return;
    }
    const current = draft();
    if (!origin || !current) return;
    const p = point(event);
    if (current.kind === 'line') {
      const points = [...(current.points ?? []), [p.x, p.y]] as [number, number][];
      const xs = points.map((pair) => pair[0]);
      const ys = points.map((pair) => pair[1]);
      setDraft({
        ...current,
        points,
        x: Math.min(...xs),
        y: Math.min(...ys),
        w: Math.max(...xs) - Math.min(...xs),
        h: Math.max(...ys) - Math.min(...ys),
      });
      return;
    }
    setDraft({
      ...current,
      x: Math.min(origin.x, p.x),
      y: Math.min(origin.y, p.y),
      w: Math.abs(p.x - origin.x),
      h: Math.abs(p.y - origin.y),
    });
  }

  function end(event: PointerEvent): void {
    if (surface?.hasPointerCapture(event.pointerId)) {
      surface.releasePointerCapture(event.pointerId);
    }
    if (drag) {
      const next = live();
      const index = drag.index;
      drag = null;
      if (next) {
        push(next);
        setSelected(index);
      }
      setLive(null);
      return;
    }
    const shape = draft();
    origin = null;
    setDraft(null);
    if (!shape) return;
    const drawn =
      shape.kind === 'line'
        ? (shape.points?.length ?? 0) >= 2
        : shape.w >= CLICK_MIN && shape.h >= CLICK_MIN;
    if (drawn) {
      push([...shapes(), shape]);
      setSelected(shapes().length);
    }
  }

  function removeSelected(): void {
    const index = selected();
    if (index === null) return;
    push(shapes().filter((_, position) => position !== index));
    setSelected(null);
  }

  return (
    <div class="annotate-editor">
      <div class="annotate-toolbar" role="toolbar" aria-label="Annotation tools">
        <For each={TOOLS}>
          {(item) => (
            <button
              type="button"
              class="annotate-tool"
              classList={{ 'is-active': tool() === item.id }}
              aria-pressed={tool() === item.id}
              onClick={() => {
                setTool(item.id);
                setNote('');
              }}
            >
              {item.label}
            </button>
          )}
        </For>
        <Show when={tool() === 'text'}>
          <input
            class="annotate-text"
            type="text"
            aria-label="Annotation text"
            placeholder="Text, then click the photo"
            value={text()}
            onInput={(event) => setText(event.currentTarget.value)}
          />
        </Show>
        <label class="annotate-colour">
          <span>Colour</span>
          <input
            type="color"
            aria-label="Colour"
            value={props.color}
            onInput={(event) => props.onInk?.(event.currentTarget.value)}
          />
        </label>
        <button
          type="button"
          class="annotate-tool"
          onClick={undo}
          disabled={undoCount() === 0}
        >
          Undo
        </button>
        <button
          type="button"
          class="annotate-tool"
          onClick={removeSelected}
          disabled={selected() === null}
        >
          Delete
        </button>
      </div>
      <div
        class="annotate-surface"
        classList={{ 'is-empty': preview().length === 0 }}
        ref={surface}
        onPointerDown={begin}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
      >
        <Show when={preview()} fallback={<p class="crop-placeholder">Preview unavailable</p>}>
          <img src={preview()} alt="Annotation canvas" draggable={false} />
          <For each={shown()}>
            {(shape, index) => (
              <span
                class="annotate-shape"
                classList={{ 'is-selected': selected() === index() }}
                style={{
                  left: `${Math.min(shape.x, shape.x + shape.w) * 100}%`,
                  top: `${Math.min(shape.y, shape.y + shape.h) * 100}%`,
                  width: `${Math.abs(shape.w) * 100}%`,
                  height: `${Math.abs(shape.h) * 100}%`,
                }}
              >
                <Show when={selected() === index() && tool() === 'select'}>
                  <span
                    class="annotate-handle"
                    aria-hidden="true"
                    onPointerDown={(event) => {
                      if (!surface) return;
                      rect = surface.getBoundingClientRect();
                      drag = {
                        index: index(),
                        mode: 'resize',
                        start: point(event),
                        shape: { ...shape },
                      };
                      setLive(shown());
                      surface.setPointerCapture(event.pointerId);
                      event.stopPropagation();
                      event.preventDefault();
                    }}
                  />
                </Show>
              </span>
            )}
          </For>
          <Show when={draft()}>
            {(shape) => (
              <span
                class="annotate-shape is-draft"
                style={{
                  left: `${Math.min(shape().x, shape().x + shape().w) * 100}%`,
                  top: `${Math.min(shape().y, shape().y + shape().h) * 100}%`,
                  width: `${Math.abs(shape().w) * 100}%`,
                  height: `${Math.abs(shape().h) * 100}%`,
                }}
              />
            )}
          </Show>
        </Show>
      </div>
      <p class="field-help">
        {note() ||
          (tool() === 'select'
            ? 'Click a shape to select it, drag to move, and drag the corner handle to resize.'
            : 'Drag on the photo to draw; Save burns the shapes into a copy.')}
      </p>
    </div>
  );
}
