import { For, Match, Show, Switch, createEffect, createSignal } from 'solid-js';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import * as ipc from '../ipc';
import ColourPicker from './colour-picker';
import FileOrder from './file-order';
import MetadataEditor from './metadata-editor';
import PageOrganizer from './page-organizer';
import GradientEditor from './gradient-editor';
import AnnotateEditor from './annotate-editor';
import { asBool, asBoxes, asNumber, asOrder, asText } from '../fields';
import type { Box, Field, FieldValue, Shape } from '../types';
import './boxes-editor.css';

interface FieldEditorProps {
  field: Field;
  value: FieldValue;
  previewPath: string | null;
  /** The whole selection: a reorder editor lists every file, not just the preview. */
  files?: { path: string; label: string }[];
  /** The dialog's ink colour, for the annotation toolbar and its swatch. */
  ink?: string;
  onInk?: (color: string) => void;
  /** The value this field started with, for a slider's reset arrow. */
  resetValue?: FieldValue;
  onChange: (value: FieldValue) => void;
}

/** The native picker, so a background image is chosen rather than typed. */
async function pickImage(): Promise<string | null> {
  if (ipc.isMock()) return null;
  try {
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: [
        {
          name: 'Images',
          extensions: ['png', 'jpg', 'jpeg', 'webp', 'avif', 'heic', 'tif', 'tiff', 'bmp', 'gif'],
        },
      ],
    });
    return typeof picked === 'string' ? picked : null;
  } catch {
    return null;
  }
}

function clampUnit(value: number): number {
  return Math.min(1, Math.max(0, value));
}

interface BoxesEditorProps {
  value: FieldValue;
  previewPath: string | null;
  onChange: (value: FieldValue) => void;
}

type BoxHandle = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';
const BOX_MIN = 0.01;

export function BoxesEditor(props: BoxesEditorProps) {
  const [preview, setPreview] = createSignal('');
  const [draft, setDraft] = createSignal<Box | null>(null);
  const [detecting, setDetecting] = createSignal(false);
  const [note, setNote] = createSignal('');
  const [selected, setSelected] = createSignal<number | null>(null);
  const [live, setLive] = createSignal<Box[] | null>(null);
  let surface: HTMLDivElement | undefined;
  let surfaceRect: DOMRect | null = null;
  let origin: { x: number; y: number } | null = null;
  let drag: {
    index: number;
    mode: 'move' | 'resize';
    handle: BoxHandle | null;
    start: { x: number; y: number };
    box: Box;
  } | null = null;

  const boxes = (): Box[] => asBoxes(props.value);
  const shown = (): Box[] => live() ?? boxes();

  // The dialog can switch files (or, for a video, wait for the frame): the preview
  // follows the path instead of being asked for once on mount.
  createEffect(() => {
    const path = props.previewPath;
    if (!path) return;
    setPreview('');
    void ipc
      .previewThumb(path, 640)
      .then((url) => setPreview(ipc.assetUrl(url)))
      .catch(() => setNote('Preview unavailable; regions can still be entered.'));
  });

  function localPoint(event: PointerEvent, rect: DOMRect): { x: number; y: number } {
    return {
      x: clampUnit((event.clientX - rect.left) / rect.width),
      y: clampUnit((event.clientY - rect.top) / rect.height),
    };
  }

  function commit(next: Box[]): void {
    props.onChange(next);
    setLive(null);
  }

  function onPointerDown(event: PointerEvent): void {
    if (!surface || !preview()) return;
    surfaceRect = surface.getBoundingClientRect();
    if (surfaceRect.width === 0 || surfaceRect.height === 0) return;
    origin = localPoint(event, surfaceRect);
    setSelected(null);
    surface.setPointerCapture(event.pointerId);
    setDraft({ x: origin.x, y: origin.y, w: 0, h: 0 });
    event.preventDefault();
  }

  function beginRegion(event: PointerEvent, index: number): void {
    if (!surface) return;
    surfaceRect = surface.getBoundingClientRect();
    const box = boxes()[index];
    setSelected(index);
    drag = {
      index,
      mode: 'move',
      handle: null,
      start: localPoint(event, surfaceRect),
      box: { ...box },
    };
    setLive(boxes());
    surface.setPointerCapture(event.pointerId);
    event.stopPropagation();
    event.preventDefault();
  }

  function beginHandle(event: PointerEvent, index: number, handle: BoxHandle): void {
    if (!surface) return;
    surfaceRect = surface.getBoundingClientRect();
    const box = boxes()[index];
    setSelected(index);
    drag = {
      index,
      mode: 'resize',
      handle,
      start: localPoint(event, surfaceRect),
      box: { ...box },
    };
    setLive(boxes());
    surface.setPointerCapture(event.pointerId);
    event.stopPropagation();
    event.preventDefault();
  }

  function movedBox(base: Box, handle: BoxHandle, delta: { x: number; y: number }): Box {
    let { x, y, w, h } = base;
    if (handle.includes('e')) w = Math.max(BOX_MIN, base.w + delta.x);
    if (handle.includes('s')) h = Math.max(BOX_MIN, base.h + delta.y);
    if (handle.includes('w')) {
      const nextX = Math.min(base.x + base.w - BOX_MIN, Math.max(0, base.x + delta.x));
      w = base.w + (base.x - nextX);
      x = nextX;
    }
    if (handle.includes('n')) {
      const nextY = Math.min(base.y + base.h - BOX_MIN, Math.max(0, base.y + delta.y));
      h = base.h + (base.y - nextY);
      y = nextY;
    }
    w = Math.min(w, 1 - x);
    h = Math.min(h, 1 - y);
    return { x, y, w, h };
  }

  function onPointerMove(event: PointerEvent): void {
    if (drag) {
      if (!surfaceRect) return;
      const point = localPoint(event, surfaceRect);
      const delta = { x: point.x - drag.start.x, y: point.y - drag.start.y };
      const next = boxes().map((box, index) => {
        if (index !== drag?.index) return box;
        if (drag.mode === 'move') {
          return {
            ...box,
            x: Math.min(1 - box.w, Math.max(0, drag.box.x + delta.x)),
            y: Math.min(1 - box.h, Math.max(0, drag.box.y + delta.y)),
          };
        }
        return drag.handle ? movedBox(drag.box, drag.handle, delta) : box;
      });
      setLive(next);
      return;
    }
    if (!origin || !surfaceRect) return;
    const point = localPoint(event, surfaceRect);
    setDraft({
      x: Math.min(origin.x, point.x),
      y: Math.min(origin.y, point.y),
      w: Math.abs(point.x - origin.x),
      h: Math.abs(point.y - origin.y),
    });
  }

  function onPointerUp(event: PointerEvent): void {
    if (surface?.hasPointerCapture(event.pointerId)) {
      surface.releasePointerCapture(event.pointerId);
    }
    if (drag) {
      drag = null;
      const next = live();
      if (next) commit(next);
      return;
    }
    const region = draft();
    if (region && region.w > BOX_MIN && region.h > BOX_MIN) {
      commit([...boxes(), region]);
      setNote('');
    }
    setDraft(null);
    origin = null;
    surfaceRect = null;
  }

  function remove(index: number): void {
    commit(boxes().filter((_, position) => position !== index));
    setSelected(null);
  }

  async function detect(faces: boolean): Promise<void> {
    const path = props.previewPath;
    if (!path || detecting()) return;
    setDetecting(true);
    try {
      const found = await ipc.detectBoxes(path, faces);
      if (found.length === 0) {
        setNote('Nothing detected.');
      } else {
        const strokes = new Set(boxes().map((box) => `${box.x},${box.y},${box.w},${box.h}`));
        const added = found
          .map((item) => item.rect)
          .filter((rect) => !strokes.has(`${rect.x},${rect.y},${rect.w},${rect.h}`));
        props.onChange([...boxes(), ...added]);
        setNote(`${found.length} region${found.length === 1 ? '' : 's'} detected.`);
      }
    } catch (error) {
      setNote(error instanceof Error ? error.message : 'Detection failed.');
    } finally {
      setDetecting(false);
    }
  }

  const handles: BoxHandle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

  return (
    <div class="boxes-editor">
      <div class="boxes-toolbar">
        <button
          type="button"
          class="button button-quiet"
          onClick={() => void detect(true)}
          disabled={detecting()}
        >
          Detect faces
        </button>
        <button
          type="button"
          class="button button-quiet"
          onClick={() => void detect(false)}
          disabled={detecting()}
        >
          Detect text
        </button>
        <button
          type="button"
          class="button button-quiet"
          onClick={() => props.onChange([])}
          disabled={boxes().length === 0}
        >
          Clear regions
        </button>
        <span class="boxes-count">{boxes().length} marked</span>
      </div>
      <div
        class="boxes-surface"
        classList={{ 'is-empty': preview().length === 0 }}
        ref={surface}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        <Show when={preview()} fallback={<p class="boxes-placeholder">Preview unavailable</p>}>
          <img src={preview()} alt="Preview" draggable={false} />
        </Show>
        <For each={shown()}>
          {(box, index) => (
            <span
              class="boxes-region"
              classList={{ 'is-selected': selected() === index() }}
              role="group"
              aria-label={`Region ${index() + 1}`}
              style={{
                left: `${box.x * 100}%`,
                top: `${box.y * 100}%`,
                width: `${box.w * 100}%`,
                height: `${box.h * 100}%`,
              }}
              onPointerDown={(event) => beginRegion(event, index())}
            >
              <button
                type="button"
                class="boxes-remove"
                aria-label={`Remove region ${index() + 1}`}
                onPointerDown={(event) => event.stopPropagation()}
                onClick={(event) => {
                  event.stopPropagation();
                  remove(index());
                }}
              >
                ×
              </button>
              <Show when={selected() === index()}>
                <For each={handles}>
                  {(handle) => (
                    <span
                      class={`boxes-handle boxes-handle-${handle}`}
                      aria-hidden="true"
                      onPointerDown={(event) => beginHandle(event, index(), handle)}
                    />
                  )}
                </For>
              </Show>
            </span>
          )}
        </For>
        <Show when={draft()}>
          {(box) => (
            <span
              class="boxes-region is-draft"
              style={{
                left: `${box().x * 100}%`,
                top: `${box().y * 100}%`,
                width: `${box().w * 100}%`,
                height: `${box().h * 100}%`,
              }}
            />
          )}
        </Show>
      </div>
      <p class="field-help">
        Drag on the preview to mark a rectangle; click one to move or resize it.
      </p>
      <Show when={note()}>
        <p class="field-help" role="status">
          {note()}
        </p>
      </Show>
    </div>
  );
}

interface OrderEditorProps {
  value: FieldValue;
  onChange: (value: FieldValue) => void;
}

function OrderEditor(props: OrderEditorProps) {
  const pages = (): number[] => asOrder(props.value);

  function move(index: number, delta: number): void {
    const next = [...pages()];
    const target = index + delta;
    if (target < 0 || target >= next.length) return;
    const moved = next[index];
    next[index] = next[target];
    next[target] = moved;
    props.onChange(next);
  }

  function add(): void {
    const current = pages();
    const next = current.length === 0 ? 1 : Math.max(...current) + 1;
    props.onChange([...current, next]);
  }

  return (
    <div class="order-editor">
      <ol class="order-list">
        <For each={pages()}>
          {(page, index) => (
            <li class="order-item">
              <span class="order-page">{page}</span>
              <button
                type="button"
                class="button button-quiet"
                onClick={() => move(index(), -1)}
                disabled={index() === 0}
                aria-label={`Move page ${page} earlier`}
              >
                Up
              </button>
              <button
                type="button"
                class="button button-quiet"
                onClick={() => move(index(), 1)}
                disabled={index() === pages().length - 1}
                aria-label={`Move page ${page} later`}
              >
                Down
              </button>
              <button
                type="button"
                class="button button-quiet"
                onClick={() =>
                  props.onChange(pages().filter((_, position) => position !== index()))
                }
                aria-label={`Remove page ${page}`}
              >
                Remove
              </button>
            </li>
          )}
        </For>
      </ol>
      <button type="button" class="button button-quiet" onClick={add}>
        Add page
      </button>
    </div>
  );
}

export default function FieldEditor(props: FieldEditorProps) {
  const id = (): string => `field-${props.field.key}`;
  const unit = (): string => props.field.unit ?? '';
  /**
   * Editors that are a surface rather than a control take the full width, with the
   * label above them. So does a slider whose label is too long to sit beside its
   * track — the reference puts those under their label with the value at the right.
   */
  const isBlock = (): boolean => {
    const kind = props.field.kind;
    if (kind === 'boxes' || kind === 'pages' || kind === 'files' || kind === 'metadata' || kind === 'shapes') {
      return true;
    }
    return kind === 'range' && props.field.label.length > 18;
  };

  return (
    <div class="field field-editor" classList={{ 'is-block': isBlock() }}>
      <Show when={props.field.kind !== 'checkbox'}>
        <label class="field-label" for={id()}>
          {props.field.label}
        </label>
      </Show>
      <Switch>
        <Match when={props.field.kind === 'number'}>
          <span class="number-row">
            <input
              id={id()}
              type="number"
              min={props.field.min}
              max={props.field.max}
              step={props.field.step ?? 1}
              value={asNumber(props.value)}
              onInput={(event) =>
                props.onChange(
                  Number.isFinite(event.currentTarget.valueAsNumber)
                    ? event.currentTarget.valueAsNumber
                    : 0,
                )
              }
            />
            <Show when={unit()}>
              <span class="field-unit">{unit()}</span>
            </Show>
          </span>
        </Match>
        <Match when={props.field.kind === 'range'}>
          <span class="range-row">
            <input
              id={id()}
              type="range"
              min={props.field.min ?? 0}
              max={props.field.max ?? 100}
              step={props.field.step ?? 1}
              value={asNumber(props.value)}
              onInput={(event) => props.onChange(event.currentTarget.valueAsNumber)}
            />
            <output for={id()}>
              {asNumber(props.value)}
              {unit()}
            </output>
            <Show when={props.resetValue !== undefined}>
              <button
                type="button"
                class="range-reset"
                aria-label={`Reset ${props.field.label}`}
                disabled={asNumber(props.value) === asNumber(props.resetValue ?? 0)}
                onClick={() => props.onChange(asNumber(props.resetValue ?? 0))}
              >
                ↺
              </button>
            </Show>
          </span>
        </Match>
        <Match when={props.field.kind === 'text'}>
          <input
            id={id()}
            type="text"
            value={asText(props.value)}
            onInput={(event) => props.onChange(event.currentTarget.value)}
          />
        </Match>
        <Match when={props.field.kind === 'color'}>
          <ColourPicker
            label={props.field.label}
            value={asText(props.value, '#111111')}
            onChange={props.onChange}
          />
        </Match>
        <Match when={props.field.kind === 'gradient'}>
          <GradientEditor value={asText(props.value)} onChange={props.onChange} />
        </Match>
        <Match when={props.field.kind === 'file'}>
          <span class="file-row">
            <input
              id={id()}
              type="text"
              value={asText(props.value)}
              onInput={(event) => props.onChange(event.currentTarget.value)}
            />
            <button
              type="button"
              onClick={() => {
                void pickImage().then((path) => {
                  if (path) props.onChange(path);
                });
              }}
            >
              Choose…
            </button>
          </span>
        </Match>
        <Match when={props.field.kind === 'segmented'}>
          <span class="segmented" role="group" aria-label={props.field.label}>
            <For each={props.field.options ?? []}>
              {(option, index) => (
                <button
                  type="button"
                  classList={{ 'is-active': asText(props.value) === option }}
                  aria-pressed={asText(props.value) === option}
                  onClick={() => props.onChange(option)}
                >
                  {props.field.option_labels?.[index()] ?? option}
                </button>
              )}
            </For>
          </span>
        </Match>
        <Match when={props.field.kind === 'select'}>
          <select
            id={id()}
            value={asText(props.value)}
            onChange={(event) => props.onChange(event.currentTarget.value)}
          >
            <For each={props.field.options ?? []}>
              {(option, index) => (
                <option value={option}>{props.field.option_labels?.[index()] ?? option}</option>
              )}
            </For>
          </select>
        </Match>
        <Match when={props.field.kind === 'checkbox'}>
          <label class="field checkbox-row" for={id()}>
            <input
              id={id()}
              type="checkbox"
              checked={asBool(props.value)}
              onChange={(event) => props.onChange(event.currentTarget.checked)}
            />
            <span class="field-label">{props.field.label}</span>
          </label>
        </Match>
        <Match when={props.field.kind === 'pages'}>
          <PageOrganizer
            path={props.previewPath}
            value={props.value}
            onChange={props.onChange}
          />
        </Match>
        <Match when={props.field.kind === 'metadata'}>
          <MetadataEditor
            path={props.previewPath}
            value={props.value}
            onChange={props.onChange}
          />
        </Match>
        <Match when={props.field.kind === 'files'}>
          <FileOrder
            files={props.files ?? []}
            value={props.value}
            onChange={props.onChange}
          />
        </Match>
        <Match when={props.field.kind === 'boxes'}>
          <BoxesEditor
            value={props.value}
            previewPath={props.previewPath}
            onChange={props.onChange}
          />
        </Match>
        <Match when={props.field.kind === 'order'}>
          <OrderEditor value={props.value} onChange={props.onChange} />
        </Match>
        <Match when={props.field.kind === 'shapes'}>
          <AnnotateEditor
            previewPath={props.previewPath}
            value={props.value}
            color={props.ink ?? '#ff5a36'}
            onInk={props.onInk}
            onChange={(value: Shape[]) => props.onChange(value)}
          />
        </Match>
      </Switch>
      <Show when={props.field.help}>
        {(help) => <p class="field-help">{help()}</p>}
      </Show>
    </div>
  );
}
