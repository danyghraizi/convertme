import type { Settings, WheelGroup, WheelKey } from './types';

export const WHEEL_KEYS: WheelKey[] = ['shift', 'control', 'alt', 'meta'];

const LABELS: Record<WheelKey, string> = {
  shift: 'Shift',
  control: 'Ctrl',
  alt: 'Alt',
  meta: 'Meta',
};

export function keyLabel(key: string): string {
  return LABELS[key as WheelKey] ?? key;
}

/** Wheel group bound to a modifier key, or null when the key is unbound. */
export function groupForKey(key: string, settings: Settings | null): WheelGroup | null {
  const convert = settings?.convert_key ?? 'shift';
  const tools = settings?.tools_key ?? 'control';
  const actions = settings?.actions_key ?? 'alt';
  if (key === convert) return 'convert';
  if (key === tools) return 'tools';
  if (key === actions) return 'actions';
  return null;
}

export function groupLabel(group: WheelGroup): string {
  if (group === 'convert') return 'Convert to';
  if (group === 'tools') return 'File tools';
  return 'Actions';
}

/** DOM KeyboardEvent.key (or a backend drag-signal key) to a WheelKey. */
export function wheelKeyFromEventKey(key: string): WheelKey | null {
  if (key === 'Shift') return 'shift';
  if (key === 'Control') return 'control';
  if (key === 'Alt') return 'alt';
  if (key === 'Meta') return 'meta';
  return null;
}
