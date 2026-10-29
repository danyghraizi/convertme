import { For, Match, Show, Switch, createSignal, onMount } from "solid-js";
import * as ipc from "../ipc";
import { asBool, asBoxes, asNumber, asOrder, asText } from "../fields";
import type { Box, Field, FieldValue } from "../types";

interface FieldEditorProps {
  field: Field;
  value: FieldValue;
  previewPath: string | null;
  onChange: (value: FieldValue) => void;
}

function clampUnit(value: number): number {
  return Math.min(1, Math.max(0, value));
}

interface BoxesEditorProps {
  value: FieldValue;
  previewPath: string | null;
  onChange: (value: FieldValue) => void;
}

function BoxesEditor(props: BoxesEditorProps) {
  const [preview, setPreview] = createSignal("");
  const [draft, setDraft] = createSignal<Box | null>(null);
  const [detecting, setDetecting] = createSignal(false);
  const [note, setNote] = createSignal("");
  let surface: HTMLDivElement | undefined;
  let surfaceRect: DOMRect | null = null;
  let origin: { x: number; y: number } | null = null;

  const boxes = (): Box[] => asBoxes(props.value);

  onMount(() => {
    const path = props.previewPath;
    if (!path) return;
    void ipc
      .previewThumb(path, 640)
      .then((url) => setPreview(ipc.assetUrl(url)))
      .catch(() =>
        setNote("Preview unavailable; regions can still be entered."),
      );
  });

  function localPoint(
    event: PointerEvent,
    rect: DOMRect,
  ): { x: number; y: number } {
    return {
      x: clampUnit((event.clientX - rect.left) / rect.width),
      y: clampUnit((event.clientY - rect.top) / rect.height),
    };
  }

  function onPointerDown(event: PointerEvent): void {
    if (!surface || !preview()) return;
    surfaceRect = surface.getBoundingClientRect();
    if (surfaceRect.width === 0 || surfaceRect.height === 0) return;
    origin = localPoint(event, surfaceRect);
    surface.setPointerCapture(event.pointerId);
    setDraft({ x: origin.x, y: origin.y, w: 0, h: 0 });
    event.preventDefault();
  }

  function onPointerMove(event: PointerEvent): void {
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
    const rect = draft();
    if (surface?.hasPointerCapture(event.pointerId)) {
      surface.releasePointerCapture(event.pointerId);
    }
    if (rect && rect.w > 0.01 && rect.h > 0.01) {
      props.onChange([...boxes(), rect]);
      setNote("");
    }
    setDraft(null);
    origin = null;
    surfaceRect = null;
  }

  async function detect(faces: boolean): Promise<void> {
    const path = props.previewPath;
    if (!path || detecting()) return;
    setDetecting(true);
    try {
      const found = await ipc.detectBoxes(path, faces);
      if (found.length === 0) {
        setNote("Nothing detected.");
      } else {
        const strokes = new Set(
          boxes().map((box) => `${box.x},${box.y},${box.w},${box.h}`),
        );
        const added = found
          .map((item) => item.rect)
          .filter(
            (rect) => !strokes.has(`${rect.x},${rect.y},${rect.w},${rect.h}`),
          );
        props.onChange([...boxes(), ...added]);
        setNote(
          `${found.length} region${found.length === 1 ? "" : "s"} detected.`,
        );
      }
    } catch (error) {
      setNote(error instanceof Error ? error.message : "Detection failed.");
    } finally {
      setDetecting(false);
    }
  }

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
        classList={{ "is-empty": preview().length === 0 }}
        ref={surface}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        <Show
          when={preview()}
          fallback={<p class="boxes-placeholder">Preview unavailable</p>}
        >
          <img src={preview()} alt="Preview" draggable={false} />
        </Show>
        <For each={boxes()}>
          {(box) => (
            <span
              class="boxes-region"
              style={{
                left: `${box.x * 100}%`,
                top: `${box.y * 100}%`,
                width: `${box.w * 100}%`,
                height: `${box.h * 100}%`,
              }}
            />
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
      <p class="field-help">Drag on the preview to mark a rectangle.</p>
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
                  props.onChange(
                    pages().filter((_, position) => position !== index()),
                  )
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
  const unit = (): string => props.field.unit ?? "";

  return (
    <div class="field field-editor">
      <Show when={props.field.kind !== "checkbox"}>
        <label class="field-label" for={id()}>
          {props.field.label}
        </label>
      </Show>
      <Switch>
        <Match when={props.field.kind === "number"}>
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
        <Match when={props.field.kind === "range"}>
          <span class="range-row">
            <input
              id={id()}
              type="range"
              min={props.field.min ?? 0}
              max={props.field.max ?? 100}
              step={props.field.step ?? 1}
              value={asNumber(props.value)}
              onInput={(event) =>
                props.onChange(event.currentTarget.valueAsNumber)
              }
            />
            <output for={id()}>
              {asNumber(props.value)}
              {unit()}
            </output>
          </span>
        </Match>
        <Match when={props.field.kind === "text"}>
          <input
            id={id()}
            type="text"
            value={asText(props.value)}
            onInput={(event) => props.onChange(event.currentTarget.value)}
          />
        </Match>
        <Match when={props.field.kind === "color"}>
          <span class="color-row">
            <input
              id={id()}
              type="color"
              value={asText(props.value, "#111111")}
              onInput={(event) => props.onChange(event.currentTarget.value)}
            />
            <output>{asText(props.value)}</output>
          </span>
        </Match>
        <Match when={props.field.kind === "select"}>
          <select
            id={id()}
            value={asText(props.value)}
            onChange={(event) => props.onChange(event.currentTarget.value)}
          >
            <For each={props.field.options ?? []}>
              {(option, index) => (
                <option value={option}>
                  {props.field.option_labels?.[index()] ?? option}
                </option>
              )}
            </For>
          </select>
        </Match>
        <Match when={props.field.kind === "checkbox"}>
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
        <Match when={props.field.kind === "boxes"}>
          <BoxesEditor
            value={props.value}
            previewPath={props.previewPath}
            onChange={props.onChange}
          />
        </Match>
        <Match when={props.field.kind === "order"}>
          <OrderEditor value={props.value} onChange={props.onChange} />
        </Match>
      </Switch>
    </div>
  );
}
