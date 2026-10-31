import type {
  Box,
  Field,
  FieldKind,
  FieldValue,
  MetaFields,
  PagesValue,
  Segment,
  Shape,
} from './types';

/**
 * Fields arrive as JSON from the Rust table. Everything the editor renders has to be
 * carried across: dropping `kind` silently turned the gradient editor, the ratio
 * buttons and the file picker into text boxes, and dropping `show_if`/`option_labels`
 * showed every fill field at once, with raw values in the menus.
 *
 * Kept in its own module so it can be tested without a window (`src/field-parse.test.ts`).
 */

const FIELD_KINDS: readonly string[] = [
  'number',
  'text',
  'range',
  'color',
  'gradient',
  'segmented',
  'file',
  'select',
  'checkbox',
  'boxes',
  'order',
  'shapes',
  'pages',
  'files',
  'metadata',
];

const SHAPE_KINDS: readonly string[] = [
  'rect',
  'arrow',
  'ellipse',
  'line',
  'highlight',
  'number',
  'text',
];

const str = (value: unknown, fallback = ''): string =>
  typeof value === 'string' ? value : fallback;

const num = (value: unknown, fallback = 0): number =>
  typeof value === 'number' && Number.isFinite(value) ? value : fallback;

const bool = (value: unknown, fallback = false): boolean =>
  typeof value === 'boolean' ? value : fallback;

function strings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : [];
}

function records(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value)
    ? value.filter(
        (item): item is Record<string, unknown> => typeof item === 'object' && item !== null,
      )
    : [];
}

function readBoxes(value: unknown): Box[] {
  return records(value)
    .map((o) => ({ x: num(o.x), y: num(o.y), w: num(o.w), h: num(o.h) }))
    .filter((box) => box.w > 0 && box.h > 0);
}

function numbers(value: unknown): number[] {
  return Array.isArray(value)
    ? value.filter((item): item is number => typeof item === 'number' && Number.isFinite(item))
    : [];
}

/** Annotations arrive as records; a shape without a known kind or a finite box is
 *  dropped rather than drawn at the origin (which reads as a corrupted photo). */
function readShapes(value: unknown): Shape[] {
  return records(value)
    .filter((o) => SHAPE_KINDS.includes(str(o.kind)))
    .map((o) => {
      const shape: Shape = {
        kind: str(o.kind) as Shape['kind'],
        x: num(o.x),
        y: num(o.y),
        w: num(o.w),
        h: num(o.h),
      };
      if (Array.isArray(o.points)) {
        shape.points = o.points
          .filter((pair): pair is unknown[] => Array.isArray(pair) && pair.length >= 2)
          .map((pair) => [num(pair[0]), num(pair[1])] as [number, number]);
      }
      if (typeof o.text === 'string') shape.text = o.text;
      if (typeof o.color === 'string') shape.color = o.color;
      if (typeof o.size === 'number' && Number.isFinite(o.size)) shape.size = o.size;
      return shape;
    });
}

function readPages(value: unknown): PagesValue {
  if (typeof value !== 'object' || value === null) return { order: [], rotate: [] };
  const o = value as Record<string, unknown>;
  return { order: numbers(o.order), rotate: numbers(o.rotate) };
}

function readMetadata(value: unknown): MetaFields {
  const result: MetaFields = { set: {}, remove: [], strip: false };
  if (typeof value !== 'object' || value === null) return result;
  const o = value as Record<string, unknown>;
  if (typeof o.set === 'object' && o.set !== null) {
    for (const [key, text] of Object.entries(o.set as Record<string, unknown>)) {
      if (typeof text === 'string') result.set[key] = text;
    }
  }
  result.remove = strings(o.remove);
  result.strip = bool(o.strip, false);
  return result;
}

function readFieldValue(kind: string, value: unknown): FieldValue {
  switch (kind) {
    case 'number':
    case 'range':
      return num(value, 0);
    case 'checkbox':
      return bool(value, false);
    case 'boxes':
      return readBoxes(value);
    case 'order':
    case 'files':
      return numbers(value);
    case 'shapes':
      return readShapes(value);
    case 'pages':
      return readPages(value);
    case 'metadata':
      return readMetadata(value);
    default:
      return str(value);
  }
}

export function readFields(value: unknown): Field[] {
  return records(value).map((o) => {
    const kind = str(o.kind);
    const field: Field = {
      key: str(o.key),
      label: str(o.label),
      kind: FIELD_KINDS.includes(kind) ? (kind as FieldKind) : 'text',
      default: readFieldValue(kind, o.default),
    };
    if (typeof o.min === 'number') field.min = o.min;
    if (typeof o.max === 'number') field.max = o.max;
    if (typeof o.step === 'number') field.step = o.step;
    if (typeof o.unit === 'string') field.unit = o.unit;
    if (Array.isArray(o.options)) field.options = strings(o.options);
    if (Array.isArray(o.option_labels)) field.option_labels = strings(o.option_labels);
    if (typeof o.group === 'string') field.group = o.group;
    if (typeof o.help === 'string') field.help = o.help;
    if (o.control === true) field.control = true;
    if (o.hidden === true) field.hidden = true;
    const when = o.show_if;
    if (when && typeof when === 'object') {
      const guard = when as Record<string, unknown>;
      if (typeof guard.key === 'string' && typeof guard.equals === 'string') {
        field.show_if = { key: guard.key, equals: guard.equals };
      }
    }
    return field;
  });
}

/**
 * One wheel segment. `title`/`action`/`benefits`/`eyebrow` are the dialog's copy:
 * dropping them renders a sheet with no heading and a rail with no words, which is
 * what the two-pane layout is made of, so the reader carries all five keys.
 */
export function readSegmentPayload(raw: unknown): Segment {
  if (typeof raw !== 'object' || raw === null) return { id: '', label: '' };
  const o = raw as Record<string, unknown>;
  const segment: Segment = { id: str(o.id), label: str(o.label) };
  if (typeof o.blurb === 'string') segment.blurb = o.blurb;
  if (typeof o.title === 'string') segment.title = o.title;
  if (typeof o.action === 'string') segment.action = o.action;
  const benefits = strings(o.benefits);
  if (benefits.length > 0) segment.benefits = benefits;
  if (typeof o.eyebrow === 'string') segment.eyebrow = o.eyebrow;
  if (typeof o.disabled === 'boolean') segment.disabled = o.disabled;
  if (typeof o.hint === 'string') segment.hint = o.hint;
  return segment;
}

export function readSegments(value: unknown): Segment[] {
  return Array.isArray(value) ? value.map(readSegmentPayload) : [];
}
