import type { Box, Field, FieldValue, MetaFields, PagesValue, Shape } from './types';

export function isBox(value: unknown): value is Box {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = value as { x?: unknown; y?: unknown; w?: unknown; h?: unknown };
  return (
    typeof candidate.x === 'number' &&
    typeof candidate.y === 'number' &&
    typeof candidate.w === 'number' &&
    typeof candidate.h === 'number'
  );
}

export function asNumber(value: FieldValue, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

export function asText(value: FieldValue, fallback = ''): string {
  return typeof value === 'string' ? value : fallback;
}

export function asBool(value: FieldValue, fallback = false): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

export function asOrder(value: FieldValue): number[] {
  return Array.isArray(value) ? value.filter((item): item is number => typeof item === 'number') : [];
}

export function asBoxes(value: FieldValue): Box[] {
  return Array.isArray(value) ? value.filter(isBox) : [];
}

const SHAPE_KINDS = new Set<Shape['kind']>([
  'rect',
  'arrow',
  'ellipse',
  'line',
  'highlight',
  'number',
  'text',
]);

export function isShape(value: unknown): value is Shape {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.kind === 'string' &&
    SHAPE_KINDS.has(candidate.kind as Shape['kind']) &&
    ['x', 'y', 'w', 'h'].every((key) => typeof candidate[key] === 'number')
  );
}

export function asShapes(value: FieldValue): Shape[] {
  return Array.isArray(value) ? value.filter(isShape) : [];
}

export function asPages(value: FieldValue): PagesValue {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return { order: [], rotate: [] };
  }
  const candidate = value as Partial<PagesValue>;
  return { order: asOrder(candidate.order ?? []), rotate: asOrder(candidate.rotate ?? []) };
}

export function asMeta(value: FieldValue): MetaFields {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return { set: {}, remove: [], strip: false };
  }
  const candidate = value as Partial<MetaFields>;
  return {
    set: typeof candidate.set === 'object' && candidate.set !== null ? candidate.set : {},
    remove: Array.isArray(candidate.remove) ? candidate.remove : [],
    strip: candidate.strip === true,
  };
}

/** Seed value for a field, coerced to the shape its editor expects. */
export function initialValue(field: Field): FieldValue {
  switch (field.kind) {
    case 'number':
    case 'range':
      return asNumber(field.default);
    case 'checkbox':
      return asBool(field.default);
    case 'boxes':
      return asBoxes(field.default);
    case 'order':
    case 'files':
      return asOrder(field.default);
    case 'shapes':
      return asShapes(field.default);
    case 'pages':
      return asPages(field.default);
    case 'metadata':
      return asMeta(field.default);
    default:
      return asText(field.default);
  }
}

/** Visibility of a field given a value map (`show_if`). */
export function visibleIn(field: Field, values: Record<string, FieldValue>): boolean {
  const rule = field.show_if;
  if (!rule) return true;
  return String(values[rule.key] ?? '') === rule.equals;
}

/**
 * The parameters a tool receives. Controls (a mode switch) and fields hidden by their
 * `show_if` never travel: the backend rejects unknown keys, and a hidden field would send
 * a value the dialog is not showing.
 */
export function collectParams(
  fields: Field[],
  values: Record<string, FieldValue>,
): Record<string, unknown> {
  const params: Record<string, unknown> = {};
  for (const field of fields) {
    if (!visibleIn(field, values) || field.control) continue;
    const value = values[field.key];
    // The metadata editor is one control over three backend keys; the page grid is
    // one control over `order` and `rotate`. Both travel flat, as the engine reads
    // them, and an empty part is omitted so the engine applies its own default.
    if (field.kind === 'metadata') {
      const meta = asMeta(value);
      if (Object.keys(meta.set).length > 0) params.set = meta.set;
      if (meta.remove.length > 0) params.remove = meta.remove;
      params.strip = meta.strip;
      continue;
    }
    if (field.kind === 'pages') {
      const pages = asPages(value);
      params.order = pages.order;
      const rotate: Record<string, number> = {};
      pages.rotate.forEach((degrees, position) => {
        if (degrees % 360 !== 0) rotate[String(position + 1)] = degrees;
      });
      params.rotate = rotate;
      continue;
    }
    // A select carries strings; a numeric choice (an image scale, a channel count) is a
    // number to the backend, and the value alone cannot tell the two apart.
    if (field.kind === 'select' && typeof value === 'string' && value.trim() !== '') {
      const numeric = Number(value);
      params[field.key] = Number.isFinite(numeric) ? numeric : value;
      continue;
    }
    params[field.key] = value;
  }
  return params;
}
