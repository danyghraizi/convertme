import type { Box, Field, FieldKind, FieldValue } from './types';

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
      return Array.isArray(value)
        ? value.filter((item): item is number => typeof item === 'number')
        : [];
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
    if (o.control === true) field.control = true;
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
