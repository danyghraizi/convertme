import type { Box, Field, FieldValue } from './types';

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
      return asOrder(field.default);
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
