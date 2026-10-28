/** Stable union of paths, preserving order and dropping duplicates. */
export function mergePaths(current: string[], next: string[]): string[] {
  const seen = new Set(current);
  const merged = [...current];
  for (const path of next) {
    if (seen.has(path)) continue;
    seen.add(path);
    merged.push(path);
  }
  return merged;
}
