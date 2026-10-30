export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** Lowercase extension without the dot, or '' when there is none. */
export function extensionOf(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf('.');
  return dot < 0 ? '' : name.slice(dot + 1).toLowerCase();
}

/**
 * Image encoders that ignore `-quality`. Mirrors the inverse of `is_lossy_ext` in
 * `crates/convertme-engines/src/image.rs`; the engine pins that list in a test.
 */
const LOSSLESS_IMAGE_EXTENSIONS: Record<string, true> = {
  png: true,
  bmp: true,
  tif: true,
  tiff: true,
  svg: true,
  ico: true,
};

/** True when the quality slider cannot change the encoded bytes. */
export function isLosslessImage(path: string): boolean {
  return LOSSLESS_IMAGE_EXTENSIONS[extensionOf(path)] === true;
}

/** True when every selected file is lossless (an empty selection is not). */
export function isLosslessSelection(paths: string[]): boolean {
  return paths.length > 0 && paths.every(isLosslessImage);
}

/** The backend stores seconds; fixtures may use milliseconds. */
export function epochMillis(value: number): number {
  return value < 1e12 ? value * 1000 : value;
}

export function formatTime(value: number | null): string {
  if (value === null) return '\u2014';
  const date = new Date(epochMillis(value));
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}
