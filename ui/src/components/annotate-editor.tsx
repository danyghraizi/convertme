import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import * as ipc from '../ipc';
import { asShapes } from '../fields';
import type { FieldValue, Shape } from '../types';
import './annotate-editor.css';

type ToolName =
  | 'select'
  | 'eraser'
  | 'pen'
  | 'arrow'
  | 'rect'
  | 'ellipse'
  | 'highlight'
  | 'text'
  | 'number';
type FontFamily = 'sans' | 'serif' | 'mono';
/** The dialog offers a family; `types.ts` is shared, so the extra key is local. */
type AnnotateShape = Shape & { font?: FontFamily };
type Corner = 'nw' | 'ne' | 'sw' | 'se';
type Point = { x: number; y: number };
type Bounds = { left: number; top: number; right: number; bottom: number; width: number; height: number };
type TextEditor = {
  index: number | null;
  x: number;
  y: number;
  value: string;
  size: number;
  font: FontFamily;
  color: string;
};

interface AnnotateEditorProps {
  previewPath: string | null;
  value: FieldValue;
  /** The dialog's ink colour: also the default for a new shape. */
  color: string;
  onInk?: (color: string) => void;
  onChange: (value: Shape[]) => void;
}

const TOOLS: { id: ToolName; label: string; hint: string }[] = [
  { id: 'select', label: 'Select', hint: 'Click a shape to select it; drag to move, drag a corner to resize.' },
  { id: 'eraser', label: 'Eraser', hint: 'Click an annotation to erase it.' },
  { id: 'pen', label: 'Pen', hint: 'Drag to draw freehand.' },
  { id: 'arrow', label: 'Arrow', hint: 'Drag from the tail to the tip.' },
  { id: 'rect', label: 'Rectangle', hint: 'Drag to draw a rectangle.' },
  { id: 'ellipse', label: 'Ellipse', hint: 'Drag to draw an ellipse.' },
  { id: 'highlight', label: 'Highlight', hint: 'Drag to highlight an area.' },
  { id: 'text', label: 'Text', hint: 'Click the photo, then type.' },
  { id: 'number', label: 'Number', hint: 'Click the photo to place a number, or drag to size it.' },
];

const FONTS: { id: FontFamily; label: string; css: string }[] = [
  { id: 'sans', label: 'Sans', css: 'sans-serif' },
  { id: 'serif', label: 'Serif', css: 'serif' },
  { id: 'mono', label: 'Mono', css: 'monospace' },
];

const HANDLES: Corner[] = ['nw', 'ne', 'sw', 'se'];

const DEFAULT_STROKE = 6;
const DEFAULT_TEXT = 40;
/** The engine's fallback for a text shape saved without a size (older values). */
const LEGACY_TEXT = 32;
const MIN_PX = 6;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function clamp01(value: number): number {
  return clamp(value, 0, 1);
}

function cssFont(family: FontFamily | undefined): string {
  return FONTS.find((item) => item.id === (family ?? 'sans'))?.css ?? 'sans-serif';
}

/** One shared 2D context, so a text hit box is measured, not guessed. */
let measureContext: CanvasRenderingContext2D | null | undefined;

function measureText(text: string, family: FontFamily | undefined, size: number): number {
  if (measureContext === undefined) {
    try {
      measureContext = typeof document === 'undefined' ? null : document.createElement('canvas').getContext('2d');
    } catch {
      measureContext = null;
    }
  }
  if (measureContext) {
    measureContext.font = `${Math.max(1, size)}px ${cssFont(family)}`;
    const width = measureContext.measureText(text || ' ').width;
    if (Number.isFinite(width) && width > 0) return width;
  }
  return Math.max(1, text.length) * Math.max(1, size) * 0.6;
}

export default function AnnotateEditor(props: AnnotateEditorProps) {
  const [preview, setPreview] = createSignal('');
  const [note, setNote] = createSignal('');
  const [natural, setNatural] = createSignal({ w: 1000, h: 1000 });
  const [tool, setTool] = createSignal<ToolName>('rect');
  const [selected, setSelected] = createSignal<number | null>(null);
  const [draft, setDraft] = createSignal<AnnotateShape | null>(null);
  const [live, setLive] = createSignal<AnnotateShape[] | null>(null);
  const [strokeSize, setStrokeSize] = createSignal(DEFAULT_STROKE);
  const [textSize, setTextSize] = createSignal(DEFAULT_TEXT);
  const [font, setFont] = createSignal<FontFamily>('sans');
  const [ink, setInk] = createSignal(props.color);
  const [editor, setEditor] = createSignal<TextEditor | null>(null);
  const [canUndo, setCanUndo] = createSignal(false);
  const [canRedo, setCanRedo] = createSignal(false);

  let surface: HTMLDivElement | undefined;
  let frame: HTMLDivElement | undefined;
  let frameRect: DOMRect | null = null;
  let origin: Point | null = null;
  let undoStack: AnnotateShape[][] = [];
  let redoStack: AnnotateShape[][] = [];
  let drag: {
    index: number;
    mode: 'move' | 'resize';
    corner: Corner | null;
    start: Point;
    shape: AnnotateShape;
    box: Bounds;
    moved: boolean;
  } | null = null;

  const shapes = (): AnnotateShape[] => asShapes(props.value) as AnnotateShape[];
  const shown = (): AnnotateShape[] => live() ?? shapes();

  const selectedShape = createMemo<AnnotateShape | null>(() => {
    const index = selected();
    if (index === null) return null;
    return shown()[index] ?? null;
  });

  const textContext = createMemo(() => {
    const shape = selectedShape();
    if (shape) return shape.kind === 'text';
    return selected() === null && tool() === 'text';
  });

  const sizeLabel = () => (textContext() ? 'Text size' : 'Stroke');
  const sizeMax = () => (textContext() ? 200 : 64);
  const sizeValue = () => {
    const shape = selectedShape();
    if (shape) {
      if (shape.kind === 'text') return clamp(shape.size ?? LEGACY_TEXT, 1, 200);
      if (shape.kind === 'number') return strokeSize();
      return clamp(shape.size ?? 4, 1, 64);
    }
    return tool() === 'text' ? textSize() : strokeSize();
  };
  const sizeEnabled = () => selectedShape()?.kind !== 'number';
  const fontEnabled = () => textContext();
  const pickerColour = () => selectedShape()?.color ?? ink();
  const pickerFont = (): FontFamily => {
    const shape = selectedShape();
    return shape?.kind === 'text' ? shape.font ?? 'sans' : font();
  };

  createEffect(() => setInk(props.color));

  createEffect(() => {
    const path = props.previewPath;
    let alive = true;
    if (!path) {
      setPreview('');
      setNote('Preview unavailable; the annotations still travel with the job.');
      return;
    }
    setNote('');
    void ipc
      .previewThumb(path, 1024)
      .then((url) => {
        if (!alive) return;
        const src = ipc.assetUrl(url);
        const image = new Image();
        image.onload = () => {
          if (alive) {
            setNatural({
              w: image.naturalWidth || 1000,
              h: image.naturalHeight || 1000,
            });
          }
        };
        image.src = src;
        setPreview(src);
        void ipc
          .imageSize(path)
          .then((size) => {
            if (alive && size && size.w > 0 && size.h > 0) setNatural(size);
          })
          .catch(() => undefined);
      })
      .catch(() => {
        if (!alive) return;
        setPreview('');
        setNote('Preview unavailable.');
      });
    onCleanup(() => {
      alive = false;
    });
  });

  onCleanup(() => {
    setPreview('');
    setEditor(null);
  });

  function syncHistory(): void {
    setCanUndo(undoStack.length > 0);
    setCanRedo(redoStack.length > 0);
  }

  function snapshot(list: AnnotateShape[]): AnnotateShape[] {
    return list.map((shape) => ({
      ...shape,
      points: shape.points?.map(([x, y]) => [x, y] as [number, number]),
    }));
  }

  function commit(next: AnnotateShape[]): void {
    undoStack = [...undoStack, snapshot(shapes())].slice(-60);
    redoStack = [];
    syncHistory();
    setLive(null);
    props.onChange(next);
  }

  function undo(): void {
    const previous = undoStack.pop();
    syncHistory();
    if (!previous) return;
    redoStack = [...redoStack, snapshot(shapes())].slice(-60);
    syncHistory();
    setSelected(null);
    setLive(null);
    props.onChange(previous);
  }

  function redo(): void {
    const next = redoStack.pop();
    syncHistory();
    if (!next) return;
    undoStack = [...undoStack, snapshot(shapes())].slice(-60);
    syncHistory();
    setSelected(null);
    setLive(null);
    props.onChange(next);
  }

  function updateShape(index: number, next: AnnotateShape): void {
    const list = shapes().slice();
    if (!list[index]) return;
    list[index] = next;
    setLive(null);
    props.onChange(list);
  }

  function nextNumber(): string {
    let highest = 0;
    for (const shape of shown()) {
      if (shape.kind !== 'number') continue;
      const value = Number.parseInt(shape.text ?? '', 10);
      if (Number.isFinite(value) && value > highest) highest = value;
    }
    return String(highest + 1);
  }

  function strokeWidth(shape: AnnotateShape): number {
    return clamp(shape.size ?? 4, 1, 64);
  }

  function bboxOf(shape: AnnotateShape): Bounds {
    const left = Math.min(shape.x, shape.x + shape.w);
    const right = Math.max(shape.x, shape.x + shape.w);
    const top = Math.min(shape.y, shape.y + shape.h);
    const bottom = Math.max(shape.y, shape.y + shape.h);
    return { left, top, right, bottom, width: right - left, height: bottom - top };
  }

  function textBox(text: string, size: number, family: FontFamily | undefined, x: number, y: number) {
    const image = natural();
    const width = image.w || 1000;
    const height = image.h || 1000;
    const w = clamp(measureText(text, family, size) / width, 1 / width, 1);
    const h = clamp((size * 1.2) / height, 1 / height, 1);
    return {
      x: clamp(x, 0, 1 - w),
      y: clamp(y, 0, 1 - h),
      w,
      h,
    };
  }

  /** The stored box, or a measured one for a legacy text shape saved with w/h 0. */
  function hitBox(shape: AnnotateShape): Bounds {
    const stored = bboxOf(shape);
    if (shape.kind === 'text' && (stored.width < 1e-6 || stored.height < 1e-6)) {
      const box = textBox(shape.text ?? '', shape.size ?? LEGACY_TEXT, shape.font, shape.x, shape.y);
      return {
        left: box.x,
        top: box.y,
        right: box.x + box.w,
        bottom: box.y + box.h,
        width: box.w,
        height: box.h,
      };
    }
    return stored;
  }

  function point(event: { clientX: number; clientY: number }): Point {
    const bounds = frameRect ?? frame?.getBoundingClientRect();
    if (!bounds || bounds.width === 0 || bounds.height === 0) return { x: 0, y: 0 };
    return {
      x: clamp01((event.clientX - bounds.left) / bounds.width),
      y: clamp01((event.clientY - bounds.top) / bounds.height),
    };
  }

  function capture(event: PointerEvent): void {
    try {
      surface?.setPointerCapture(event.pointerId);
    } catch {
      // Synthetic pointer ids (and a surface that left the DOM) cannot be captured.
    }
  }

  function release(event: PointerEvent): void {
    try {
      if (surface?.hasPointerCapture(event.pointerId)) surface.releasePointerCapture(event.pointerId);
    } catch {
      // The pointer is already gone; releasing it again must not throw.
    }
  }

  function nearSegment(a: Point, b: Point, p: Point, tolerance: number): boolean {
    const image = natural();
    const width = image.w || 1000;
    const height = image.h || 1000;
    const ax = a.x * width;
    const ay = a.y * height;
    const bx = b.x * width;
    const by = b.y * height;
    const px = p.x * width;
    const py = p.y * height;
    const dx = bx - ax;
    const dy = by - ay;
    const length = dx * dx + dy * dy;
    const t = length > 0 ? clamp(((px - ax) * dx + (py - ay) * dy) / length, 0, 1) : 0;
    return Math.hypot(px - (ax + t * dx), py - (ay + t * dy)) <= tolerance;
  }

  function hitTest(list: AnnotateShape[], p: Point): number | null {
    for (let index = list.length - 1; index >= 0; index -= 1) {
      const shape = list[index];
      const tolerance = strokeWidth(shape) + 8;
      if (shape.kind === 'line') {
        const points =
          shape.points && shape.points.length > 1
            ? shape.points
            : [
                [shape.x, shape.y],
                [shape.x + shape.w, shape.y + shape.h],
              ];
        for (let i = 1; i < points.length; i += 1) {
          if (
            nearSegment(
              { x: points[i - 1][0], y: points[i - 1][1] },
              { x: points[i][0], y: points[i][1] },
              p,
              tolerance,
            )
          ) {
            return index;
          }
        }
        continue;
      }
      if (shape.kind === 'arrow') {
        if (
          nearSegment(
            { x: shape.x, y: shape.y },
            { x: shape.x + shape.w, y: shape.y + shape.h },
            p,
            tolerance + 6,
          )
        ) {
          return index;
        }
        continue;
      }
      const box = hitBox(shape);
      if (p.x >= box.left && p.x <= box.right && p.y >= box.top && p.y <= box.bottom) {
        return index;
      }
    }
    return null;
  }

  function translate(shape: AnnotateShape, dx: number, dy: number): AnnotateShape {
    const box = bboxOf(shape);
    const moveX = clamp(dx, -box.left, 1 - box.right);
    const moveY = clamp(dy, -box.top, 1 - box.bottom);
    return {
      ...shape,
      x: clamp01(shape.x + moveX),
      y: clamp01(shape.y + moveY),
      points: shape.points?.map(([x, y]) => [clamp01(x + moveX), clamp01(y + moveY)] as [number, number]),
    };
  }

  function resize(shape: AnnotateShape, original: Bounds, corner: Corner, p: Point): AnnotateShape {
    const image = natural();
    const width = image.w || 1000;
    const height = image.h || 1000;
    const minW = MIN_PX / width;
    const minH = MIN_PX / height;
    let { left, top, right, bottom } = original;
    if (corner === 'nw' || corner === 'sw') left = clamp(p.x, 0, right - minW);
    else right = clamp(p.x, left + minW, 1);
    if (corner === 'nw' || corner === 'ne') top = clamp(p.y, 0, bottom - minH);
    else bottom = clamp(p.y, top + minH, 1);

    if (shape.kind === 'text') {
      const oldWidth = Math.max(original.width, 1e-6);
      const oldHeight = Math.max(original.height, 1e-6);
      const scale = Math.max((right - left) / oldWidth, (bottom - top) / oldHeight);
      const size = clamp(Math.round((shape.size ?? LEGACY_TEXT) * scale), 1, 200);
      const measured = textBox(shape.text ?? '', size, shape.font, shape.x, shape.y);
      const newWidth = measured.w;
      const newHeight = measured.h;
      const x = corner === 'nw' || corner === 'sw' ? original.right - newWidth : original.left;
      const y = corner === 'nw' || corner === 'ne' ? original.bottom - newHeight : original.top;
      return {
        ...shape,
        size,
        x: clamp(x, 0, 1 - newWidth),
        y: clamp(y, 0, 1 - newHeight),
        w: newWidth,
        h: newHeight,
      };
    }

    if (shape.kind === 'line' && shape.points && shape.points.length >= 2) {
      const oldWidth = original.width;
      const oldHeight = original.height;
      const mapX = (x: number) =>
        oldWidth > 1e-6 ? left + ((x - original.left) / oldWidth) * (right - left) : (left + right) / 2;
      const mapY = (y: number) =>
        oldHeight > 1e-6 ? top + ((y - original.top) / oldHeight) * (bottom - top) : (top + bottom) / 2;
      const points = shape.points.map(
        ([x, y]) => [clamp01(mapX(x)), clamp01(mapY(y))] as [number, number],
      );
      return { ...shape, ...boundsOfPoints(points), points };
    }

    if (shape.kind === 'arrow' || shape.kind === 'line') {
      const oldWidth = original.width;
      const oldHeight = original.height;
      const mapX = (x: number) =>
        oldWidth > 1e-6 ? left + ((x - original.left) / oldWidth) * (right - left) : (left + right) / 2;
      const mapY = (y: number) =>
        oldHeight > 1e-6 ? top + ((y - original.top) / oldHeight) * (bottom - top) : (top + bottom) / 2;
      const startX = mapX(shape.x);
      const startY = mapY(shape.y);
      const endX = mapX(shape.x + shape.w);
      const endY = mapY(shape.y + shape.h);
      return { ...shape, x: startX, y: startY, w: endX - startX, h: endY - startY };
    }

    return { ...shape, x: left, y: top, w: right - left, h: bottom - top };
  }

  function boundsOfPoints(points: [number, number][]): { x: number; y: number; w: number; h: number } {
    const xs = points.map(([x]) => x);
    const ys = points.map(([, y]) => y);
    const left = Math.min(...xs);
    const top = Math.min(...ys);
    return { x: left, y: top, w: Math.max(...xs) - left, h: Math.max(...ys) - top };
  }

  function openEditor(index: number | null, at: Point, initial: string, size: number, family: FontFamily, color: string): void {
    setSelected(index);
    setEditor({ index, x: at.x, y: at.y, value: initial, size, font: family, color });
  }

  function cancelEditor(): void {
    if (!editor()) return;
    setEditor(null);
  }

  function commitEditor(): void {
    const current = editor();
    if (!current) return;
    setEditor(null);
    const value = current.value.trim();
    if (!value) {
      if (current.index !== null) {
        const list = shapes();
        if (list[current.index]) {
          commit(list.filter((_, index) => index !== current.index));
          setSelected(null);
        }
      }
      return;
    }
    const size = clamp(current.size, 1, 200);
    const box = textBox(value, size, current.font, current.x, current.y);
    if (current.index !== null) {
      const list = shapes();
      const existing = list[current.index];
      if (!existing) return;
      const next = list.slice();
      next[current.index] = {
        ...existing,
        text: value,
        size,
        font: current.font,
        x: box.x,
        y: box.y,
        w: box.w,
        h: box.h,
      };
      commit(next);
      setSelected(current.index);
      return;
    }
    const shape: AnnotateShape = {
      kind: 'text',
      x: box.x,
      y: box.y,
      w: box.w,
      h: box.h,
      text: value,
      color: current.color,
      size,
      font: current.font,
    };
    const next = [...shapes(), shape];
    commit(next);
    setSelected(next.length - 1);
    setNote('');
  }

  function editSelectedText(): void {
    const index = selected();
    if (index === null) return;
    const shape = shapes()[index];
    if (!shape || shape.kind !== 'text') return;
    openEditor(
      index,
      { x: shape.x, y: shape.y },
      shape.text ?? '',
      shape.size ?? LEGACY_TEXT,
      shape.font ?? font(),
      shape.color ?? ink(),
    );
  }

  function defaultNumberBox(p: Point): { x: number; y: number; w: number; h: number } {
    const image = natural();
    const width = image.w || 1000;
    const height = image.h || 1000;
    const diameter = Math.max(8, Math.min(width, height) * 0.06);
    const w = Math.min(1, diameter / width);
    const h = Math.min(1, diameter / height);
    return {
      x: clamp(p.x - w / 2, 0, 1 - w),
      y: clamp(p.y - h / 2, 0, 1 - h),
      w,
      h,
    };
  }

  function begin(event: PointerEvent): void {
    if (!surface || !frame || !preview()) return;
    frameRect = frame.getBoundingClientRect();
    if (frameRect.width === 0 || frameRect.height === 0) return;
    // A pointer action elsewhere commits the inline editor instead of swallowing the
    // click; the field then behaves like a normal canvas again.
    if (editor()) commitEditor();
    const p = point(event);
    const active = tool();

    if (active === 'select') {
      const list = shapes();
      const hit = hitTest(list, p);
      setSelected(hit);
      setLive(null);
      if (hit !== null) {
        drag = {
          index: hit,
          mode: 'move',
          corner: null,
          start: p,
          shape: { ...list[hit] },
          box: hitBox(list[hit]),
          moved: false,
        };
        capture(event);
      }
      event.preventDefault();
      return;
    }

    if (active === 'eraser') {
      const list = shapes();
      const hit = hitTest(list, p);
      if (hit !== null) {
        commit(list.filter((_, index) => index !== hit));
        setSelected(null);
        setNote('');
      }
      event.preventDefault();
      return;
    }

    if (active === 'text') {
      openEditor(null, p, '', textSize(), font(), ink());
      event.preventDefault();
      return;
    }

    const colour = ink();
    let shape: AnnotateShape;
    if (active === 'pen') {
      shape = {
        kind: 'line',
        x: p.x,
        y: p.y,
        w: 0,
        h: 0,
        points: [[p.x, p.y]],
        color: colour,
        size: strokeSize(),
      };
    } else if (active === 'number') {
      shape = {
        kind: 'number',
        ...defaultNumberBox(p),
        text: nextNumber(),
        color: colour,
      };
    } else {
      shape = {
        kind: active,
        x: p.x,
        y: p.y,
        w: 0,
        h: 0,
        color: colour,
        size: strokeSize(),
      };
    }
    origin = p;
    setDraft(shape);
    capture(event);
    event.preventDefault();
  }

  function move(event: PointerEvent): void {
    if (drag) {
      const current = drag;
      const p = point(event);
      const dx = p.x - current.start.x;
      const dy = p.y - current.start.y;
      if (Math.abs(dx) > 1e-5 || Math.abs(dy) > 1e-5) current.moved = true;
      const list = shapes().map((shape, index) => {
        if (index !== current.index) return shape;
        if (current.mode === 'move') return translate(current.shape, dx, dy);
        return resize(current.shape, current.box, current.corner ?? 'se', p);
      });
      setLive(list);
      return;
    }

    const current = draft();
    if (!origin || !current) return;
    const p = point(event);
    if (current.kind === 'line') {
      const points = [...(current.points ?? []), [p.x, p.y]] as [number, number][];
      setDraft({ ...current, ...boundsOfPoints(points), points });
      return;
    }
    if (current.kind === 'number') {
      const image = natural();
      const width = image.w || 1000;
      const height = image.h || 1000;
      const distance = Math.max(Math.abs(p.x - origin.x) * width, Math.abs(p.y - origin.y) * height);
      if (distance <= 4) return;
      const diameter = Math.max(8, distance * 2);
      const w = Math.min(1, diameter / width);
      const h = Math.min(1, diameter / height);
      setDraft({
        ...current,
        x: clamp(origin.x - w / 2, 0, 1 - w),
        y: clamp(origin.y - h / 2, 0, 1 - h),
        w,
        h,
      });
      return;
    }
    if (current.kind === 'arrow') {
      setDraft({
        ...current,
        x: origin.x,
        y: origin.y,
        w: p.x - origin.x,
        h: p.y - origin.y,
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

  function drawn(shape: AnnotateShape): boolean {
    const image = natural();
    const width = image.w || 1000;
    const height = image.h || 1000;
    if (shape.kind === 'number') return shape.w > 0 && shape.h > 0;
    if (shape.kind === 'text') return true;
    if (shape.kind === 'line') {
      const points = shape.points ?? [];
      if (points.length < 2) return false;
      return points.some(([x, y]) => Math.hypot((x - points[0][0]) * width, (y - points[0][1]) * height) >= MIN_PX);
    }
    if (shape.kind === 'arrow') {
      return Math.hypot(shape.w * width, shape.h * height) >= MIN_PX;
    }
    return Math.abs(shape.w) * width >= MIN_PX && Math.abs(shape.h) * height >= MIN_PX;
  }

  function end(event: PointerEvent): void {
    release(event);
    if (drag) {
      const current = drag;
      drag = null;
      const next = live();
      setLive(null);
      if (current.moved && next) {
        commit(next);
        setSelected(current.index);
      } else {
        setSelected(current.index);
      }
      return;
    }
    const shape = draft();
    origin = null;
    setDraft(null);
    if (!shape || !drawn(shape)) return;
    const next = [...shapes(), shape];
    commit(next);
    setSelected(next.length - 1);
    setNote('');
  }

  function beginResize(event: PointerEvent, corner: Corner): void {
    if (!frame) return;
    frameRect = frame.getBoundingClientRect();
    const index = selected();
    if (index === null) return;
    const shape = shapes()[index];
    if (!shape) return;
    drag = {
      index,
      mode: 'resize',
      corner,
      start: point(event),
      shape: { ...shape },
      box: hitBox(shape),
      moved: false,
    };
    setLive(shapes());
    capture(event);
    event.stopPropagation();
    event.preventDefault();
  }

  function removeSelected(): void {
    const index = selected();
    if (index === null) return;
    const list = shapes();
    if (!list[index]) return;
    commit(list.filter((_, position) => position !== index));
    setSelected(null);
  }

  function applyColour(value: string): void {
    setInk(value);
    props.onInk?.(value);
    const index = selected();
    const shape = selectedShape();
    if (index !== null && shape) updateShape(index, { ...shape, color: value });
  }

  function applySize(value: number): void {
    const index = selected();
    const shape = selectedShape();
    if (index !== null && shape) {
      if (shape.kind === 'number') return;
      if (shape.kind === 'text') {
        const box = textBox(shape.text ?? '', value, shape.font, shape.x, shape.y);
        updateShape(index, { ...shape, size: value, ...box });
        setTextSize(value);
      } else {
        updateShape(index, { ...shape, size: value });
        setStrokeSize(value);
      }
      return;
    }
    if (tool() === 'text') setTextSize(value);
    else setStrokeSize(value);
  }

  function applyFont(family: FontFamily): void {
    setFont(family);
    const index = selected();
    const shape = selectedShape();
    if (index !== null && shape && shape.kind === 'text') {
      const box = textBox(shape.text ?? '', shape.size ?? LEGACY_TEXT, family, shape.x, shape.y);
      updateShape(index, { ...shape, font: family, ...box });
    }
  }

  function editDigit(value: string): void {
    const index = selected();
    const shape = selectedShape();
    if (index === null || !shape || shape.kind !== 'number') return;
    updateShape(index, { ...shape, text: value });
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.defaultPrevented) return;
    const target = event.target as HTMLElement | null;
    const typing =
      !!target &&
      (target.tagName === 'TEXTAREA' ||
        target.isContentEditable ||
        (target.tagName === 'INPUT' &&
          !['range', 'color', 'checkbox', 'radio'].includes(
            (target as HTMLInputElement).type,
          )));

    if (event.key === 'Escape') {
      if (editor()) {
        event.preventDefault();
        event.stopPropagation();
        cancelEditor();
        return;
      }
      if (selected() !== null) {
        event.preventDefault();
        event.stopPropagation();
        setSelected(null);
      }
      return;
    }
    if (typing) return;
    if ((event.key === 'Delete' || event.key === 'Backspace') && selected() !== null) {
      event.preventDefault();
      event.stopPropagation();
      removeSelected();
    }
  }

  onMount(() => {
    window.addEventListener('keydown', onKeyDown, true);
    onCleanup(() => window.removeEventListener('keydown', onKeyDown, true));
  });

  function drawShape(shape: AnnotateShape) {
    const image = natural();
    const width = image.w || 1000;
    const height = image.h || 1000;
    const colour = shape.color ?? ink();
    const stroke = strokeWidth(shape);
    const box = bboxOf(shape);
    const x1 = shape.x * width;
    const y1 = shape.y * height;
    const x2 = (shape.x + shape.w) * width;
    const y2 = (shape.y + shape.h) * height;
    const left = box.left * width;
    const top = box.top * height;
    const w = box.width * width;
    const h = box.height * height;
    const family = cssFont(shape.font);
    const className = 'annotate-shape';
    const kind = shape.kind;

    switch (shape.kind) {
      case 'rect':
        return (
          <rect
            class={className}
            data-shape-kind={kind}
            x={left}
            y={top}
            width={w}
            height={h}
            fill="none"
            stroke={colour}
            stroke-width={stroke}
          />
        );
      case 'ellipse':
        return (
          <ellipse
            class={className}
            data-shape-kind={kind}
            cx={left + w / 2}
            cy={top + h / 2}
            rx={w / 2}
            ry={h / 2}
            fill="none"
            stroke={colour}
            stroke-width={stroke}
          />
        );
      case 'highlight':
        return (
          <rect
            class={className}
            data-shape-kind={kind}
            x={left}
            y={top}
            width={w}
            height={h}
            fill={colour}
            fill-opacity="0.5"
          />
        );
      case 'number': {
        const size = clamp(Math.min(w, h) * 0.55, 8, 200);
        const cx = left + w / 2;
        const cy = top + h / 2;
        const radius = Math.max(1, Math.min(w, h) / 2);
        return (
          <>
            <circle
              class={className}
              data-shape-kind={kind}
              cx={cx}
              cy={cy}
              r={radius}
              fill={colour}
            />
            <text
              class={className}
              data-shape-kind={kind}
              x={cx}
              y={cy}
              text-anchor="middle"
              dominant-baseline="central"
              fill="#fff"
              font-size={`${size}`}
              font-family={family}
            >
              {shape.text ?? ''}
            </text>
          </>
        );
      }
      case 'text': {
        const size = clamp(shape.size ?? LEGACY_TEXT, 1, 200);
        return (
          <text
            class={className}
            data-shape-kind={kind}
            x={shape.x * width}
            y={shape.y * height + 0.8 * size}
            fill={colour}
            font-size={`${size}`}
            font-family={family}
          >
            {shape.text ?? ''}
          </text>
        );
      }
      case 'line': {
        const points =
          shape.points && shape.points.length > 1
            ? shape.points
            : [
                [shape.x, shape.y],
                [shape.x + shape.w, shape.y + shape.h],
              ];
        return (
          <polyline
            class={className}
            data-shape-kind={kind}
            points={points.map(([x, y]) => `${x * width},${y * height}`).join(' ')}
            fill="none"
            stroke={colour}
            stroke-width={stroke}
          />
        );
      }
      default: {
        const angle = Math.atan2(y2 - y1, x2 - x1);
        const head = clamp(0.03 * Math.min(width, height), 8, 48);
        const ax = x2 - head * Math.cos(angle + 0.45);
        const ay = y2 - head * Math.sin(angle + 0.45);
        const bx = x2 - head * Math.cos(angle - 0.45);
        const by = y2 - head * Math.sin(angle - 0.45);
        return (
          <>
            <line
              class={className}
              data-shape-kind={kind}
              x1={x1}
              y1={y1}
              x2={x2}
              y2={y2}
              stroke={colour}
              stroke-width={stroke}
            />
            <line
              class={className}
              data-shape-kind={kind}
              x1={x2}
              y1={y2}
              x2={ax}
              y2={ay}
              stroke={colour}
              stroke-width={stroke}
            />
            <line
              class={className}
              data-shape-kind={kind}
              x1={x2}
              y1={y2}
              x2={bx}
              y2={by}
              stroke={colour}
              stroke-width={stroke}
            />
          </>
        );
      }
    }
  }

  function renderScale(): number {
    const image = natural();
    if (!frame || !image.w) return 1;
    return frame.clientWidth / image.w || 1;
  }

  return (
    <div class="annotate-editor">
      <div class="annotate-toolbar" role="toolbar" aria-label="Annotation tools">
        <div class="annotate-tools">
          <For each={TOOLS}>
            {(item) => (
              <button
                type="button"
                class="annotate-tool"
                classList={{ 'is-active': tool() === item.id }}
                aria-pressed={tool() === item.id}
                onClick={() => {
                  if (editor()) commitEditor();
                  setTool(item.id);
                  setDraft(null);
                  setLive(null);
                  if (item.id !== 'select') setSelected(null);
                  setNote(item.hint);
                }}
              >
                {item.label}
              </button>
            )}
          </For>
        </div>
        <div class="annotate-properties" role="group" aria-label="Annotation properties">
        <label class="annotate-control annotate-colour">
          <span>Colour</span>
          <input
            type="color"
            aria-label="Colour"
            value={pickerColour()}
            onInput={(event) => applyColour(event.currentTarget.value)}
          />
        </label>
        <label class="annotate-control annotate-size">
          <span>{sizeLabel()}</span>
          <input
            type="range"
            aria-label="Size"
            min="1"
            max={sizeMax()}
            step="1"
            value={sizeValue()}
            disabled={!sizeEnabled()}
            onInput={(event) => applySize(Number(event.currentTarget.value))}
          />
          <output class="annotate-size-value">{Math.round(sizeValue())}</output>
        </label>
        <label class="annotate-control annotate-font">
          <span>Font</span>
          <select
            aria-label="Font"
            value={pickerFont()}
            disabled={!fontEnabled()}
            onChange={(event) => applyFont(event.currentTarget.value as FontFamily)}
          >
            <For each={FONTS}>{(item) => <option value={item.id}>{item.label}</option>}</For>
          </select>
        </label>
        <Show when={selectedShape()?.kind === 'number' ? selectedShape() : null}>
          {(shape) => (
            <label class="annotate-control annotate-digit">
              <span>Number</span>
              <input
                type="text"
                inputMode="numeric"
                aria-label="Number label"
                maxLength={3}
                value={shape().text ?? ''}
                onInput={(event) => {
                  const digits = event.currentTarget.value.replace(/[^0-9]/g, '').slice(0, 3);
                  event.currentTarget.value = digits;
                  editDigit(digits);
                }}
                onBlur={(event) => {
                  if (!event.currentTarget.value) {
                    event.currentTarget.value = '1';
                    editDigit('1');
                  }
                }}
              />
            </label>
          )}
        </Show>
        <div class="annotate-actions">
          <button type="button" class="annotate-tool" onClick={undo} disabled={!canUndo()}>
            Undo
          </button>
          <button type="button" class="annotate-tool" onClick={redo} disabled={!canRedo()}>
            Redo
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
        </div>
      </div>
      <div
        class="annotate-surface"
        classList={{ 'is-empty': preview().length === 0 }}
        ref={surface}
        onPointerDown={begin}
        onPointerMove={move}
        onPointerUp={end}
        onPointerCancel={end}
        onDblClick={(event) => {
          if (tool() !== 'select') return;
          frameRect = frame?.getBoundingClientRect() ?? null;
          const p = point(event);
          const list = shapes();
          const current = selected();
          if (current !== null && list[current]?.kind === 'text') {
            const box = hitBox(list[current]);
            if (p.x >= box.left && p.x <= box.right && p.y >= box.top && p.y <= box.bottom) {
              editSelectedText();
              event.preventDefault();
              return;
            }
          }
          const hit = hitTest(list, p);
          if (hit === null || list[hit].kind !== 'text') return;
          setSelected(hit);
          openEditor(
            hit,
            { x: list[hit].x, y: list[hit].y },
            list[hit].text ?? '',
            list[hit].size ?? LEGACY_TEXT,
            list[hit].font ?? font(),
            list[hit].color ?? ink(),
          );
          event.preventDefault();
        }}
      >
        <Show when={preview()} fallback={<p class="annotate-placeholder">Preview unavailable</p>}>
          <div class="annotate-frame" ref={frame}>
            <img src={preview()} alt="Annotation canvas" draggable={false} />
            <svg
              class="annotate-shapes"
              viewBox={`0 0 ${natural().w} ${natural().h}`}
              preserveAspectRatio="none"
              aria-hidden="true"
            >
              <For each={shown()}>{(shape) => drawShape(shape)}</For>
              <Show when={draft()}>{(shape) => drawShape(shape())}</Show>
            </svg>
            <Show when={editor()}>
              {(state) => (
                <textarea
                  class="annotate-text-editor"
                  aria-label="Annotation text"
                  rows={1}
                  value={state().value}
                  style={{
                    left: `${state().x * 100}%`,
                    top: `${state().y * 100}%`,
                    'font-size': `${Math.max(10, state().size * renderScale())}px`,
                    'font-family': cssFont(state().font),
                    color: state().color,
                  }}
                  ref={(element) => {
                    queueMicrotask(() => {
                      element.focus();
                      const end = element.value.length;
                      element.setSelectionRange(end, end);
                    });
                  }}
                  onInput={(event) => {
                    const current = editor();
                    if (current) setEditor({ ...current, value: event.currentTarget.value });
                  }}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' && !event.shiftKey) {
                      event.preventDefault();
                      commitEditor();
                    } else if (event.key === 'Escape') {
                      event.preventDefault();
                      event.stopPropagation();
                      cancelEditor();
                    }
                  }}
                  onBlur={commitEditor}
                  onPointerDown={(event) => event.stopPropagation()}
                />
              )}
            </Show>
            <Show when={selectedShape()}>
              {(shape) => {
                const box = hitBox(shape());
                return (
                  <span
                    class="annotate-selection"
                    style={{
                      left: `${box.left * 100}%`,
                      top: `${box.top * 100}%`,
                      width: `${box.width * 100}%`,
                      height: `${box.height * 100}%`,
                    }}
                  >
                    <Show when={tool() === 'select'}>
                      <For each={HANDLES}>
                        {(corner) => (
                          <span
                            class={`annotate-handle ${corner}`}
                            aria-hidden="true"
                            onPointerDown={(event) => beginResize(event, corner)}
                          />
                        )}
                      </For>
                    </Show>
                  </span>
                );
              }}
            </Show>
          </div>
        </Show>
        <Show when={note()}>
          <p class="annotate-status" role="status">
            {note()}
          </p>
        </Show>
      </div>
    </div>
  );
}
