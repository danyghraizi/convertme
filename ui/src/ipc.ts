import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import * as mock from './mock';
import { readFields, readSegmentPayload, readSegments } from './field-parse';

export { readFields, readSegmentPayload, readSegments };
import type {
  AnalyzeResult,
  BootInfo,
  CollageLayout,
  CompressPreview,
  DetectedBox,
  DragSignal,
  Field,
  HistoryRow,
  Intent,
  JobEvent,
  Loudness,
  MediaInfo,
  MetaGroup,
  PdfPage,
  RunResult,
  Settings,
  WaveformData,
  ChannelWaveforms,
  WheelContents,
} from './types';

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export interface AppEvents {
  'open-paths': string[];
  'drag-signal': DragSignal;
  'job-event': JobEvent;
  'wheel-mode': string;
  'wheel-shown': null;
  'wheel-hidden': null;
  'overlay-open': null;
  'capabilities-changed': null;
}

export interface DropPayload {
  kind: 'enter' | 'over' | 'drop' | 'leave';
  paths: string[];
}

const noopUnlisten: UnlistenFn = () => {
  /* nothing to detach outside Tauri */
};

export function isMock(): boolean {
  return typeof window === 'undefined' || typeof window.__TAURI_INTERNALS__ === 'undefined';
}

/** Window label, with a `?window=wheel` fallback for the Vite dev server. */
export function currentWindowLabel(): string {
  try {
    const label = getCurrentWindow().label;
    if (typeof label === 'string' && label.length > 0) return label;
  } catch {
    // Not running under Tauri: fall through to the query string.
  }
  const fromQuery = new URLSearchParams(window.location.search).get('window');
  return fromQuery && fromQuery.length > 0 ? fromQuery : 'main';
}

/** Absolute path or data URL suitable for an <img src>. */
export function assetUrl(path: string): string {
  if (/^(data:|https?:|blob:)/i.test(path)) return path;
  if (isMock()) return path;
  try {
    return convertFileSrc(path);
  } catch {
    return path;
  }
}

/* -------------------------------------------------------------------------- */
/* Payload readers. Everything crossing the IPC boundary starts as `unknown`.  */
/* -------------------------------------------------------------------------- */

function record(raw: unknown, what: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null) {
    throw new Error(`${what}: the backend returned ${typeof raw}, expected an object`);
  }
  return raw as Record<string, unknown>;
}

const str = (value: unknown, fallback = ''): string =>
  typeof value === 'string' ? value : fallback;

const num = (value: unknown, fallback = 0): number =>
  typeof value === 'number' && Number.isFinite(value) ? value : fallback;

const numOrNull = (value: unknown): number | null =>
  typeof value === 'number' && Number.isFinite(value) ? value : null;

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

function readSettings(raw: unknown): Settings {
  const o = record(raw, 'settings');
  const keys = ['shift', 'control', 'alt', 'meta'] as const;
  const readKey = (value: unknown, fallback: Settings['convert_key']): Settings['convert_key'] =>
    keys.find((key) => key === value) ?? fallback;
  const overwrite = o.overwrite;
  return {
    convert_key: readKey(o.convert_key, 'shift'),
    tools_key: readKey(o.tools_key, 'control'),
    actions_key: readKey(o.actions_key, 'alt'),
    overwrite:
      overwrite === 'ask' || overwrite === 'replace' || overwrite === 'never' ? overwrite : 'never',
    history_limit: num(o.history_limit, 100),
    preview_cache_bytes: num(o.preview_cache_bytes, 128 * 1024 * 1024),
    ocr_languages: strings(o.ocr_languages),
    update_check: bool(o.update_check, false),
    drag_backend: str(o.drag_backend, 'auto'),
    start_at_login: Boolean(o.start_at_login),
    redact_fill: str(o.redact_fill, '#111111'),
    extra_tools_dir: typeof o.extra_tools_dir === 'string' ? o.extra_tools_dir : null,
  };
}


function readThumbs(value: unknown): AnalyzeResult['thumbs'] {
  return records(value).map((o) => ({
    path: str(o.path),
    thumb: str(o.thumb),
    size: typeof o.size === 'number' ? o.size : 0,
  }));
}

function readSkipped(value: unknown): AnalyzeResult['skipped'] {
  return records(value).map((o) => ({ path: str(o.path), message: str(o.message) }));
}

function readHistoryRow(raw: unknown): HistoryRow {
  const o = record(raw, 'history row');
  const status = str(o.status);
  const valid =
    status === 'queued' ||
    status === 'running' ||
    status === 'done' ||
    status === 'failed' ||
    status === 'cancelled' ||
    status === 'interrupted';
  const row: HistoryRow = {
    id: str(o.id),
    input: str(o.input),
    output: typeof o.output === 'string' ? o.output : null,
    status: valid ? status : 'interrupted',
    started: numOrNull(o.started),
    ended: numOrNull(o.ended),
    intent: str(o.intent),
  };
  if (typeof o.error_code === 'string') row.error_code = o.error_code;
  if (typeof o.error_tail === 'string') row.error_tail = o.error_tail;
  return row;
}

function readRunResult(raw: unknown): RunResult {
  const o = record(raw, 'run');
  return { jobs: strings(o.jobs), skipped: readSkipped(o.skipped) };
}

function readCompressPreview(raw: unknown): CompressPreview {
  const o = record(raw, 'compress preview');
  return {
    path: str(o.path),
    bytes: num(o.bytes),
    source_bytes: num(o.source_bytes),
    width: num(o.width),
    height: num(o.height),
  };
}

function readCollageLayout(raw: unknown): CollageLayout {
  const o = record(raw, 'collage layout');
  return {
    width: num(o.width),
    height: num(o.height),
    cols: num(o.cols),
    rows: num(o.rows),
    spacing: num(o.spacing),
    cells: records(o.cells).map((cell) => ({
      input: num(cell.input),
      row: num(cell.row),
      col: num(cell.col),
      row_span: num(cell.row_span, 1),
      col_span: num(cell.col_span, 1),
      x: num(cell.x),
      y: num(cell.y),
      w: num(cell.w),
      h: num(cell.h),
    })),
  };
}

function readDetectedBoxes(value: unknown): DetectedBox[] {
  return records(value)
    .map((o) => {
      // The backend returns bare Rects; the contract shape nests them under
      // `rect` with a kind and detail, so accept both.
      const rect = typeof o.rect === 'object' && o.rect !== null ? record(o.rect, 'box') : o;
      return {
        rect: { x: num(rect.x), y: num(rect.y), w: num(rect.w), h: num(rect.h) },
        kind: str(o.kind, 'region'),
        detail: str(o.detail),
      };
    })
    .filter((box) => box.rect.w > 0 && box.rect.h > 0);
}

function readDragSignal(raw: unknown): DragSignal {
  const o = record(raw, 'drag-signal');
  const kind = str(o.kind);
  if (kind === 'modifier') return { kind, key: str(o.key), active: bool(o.active) };
  if (kind === 'shortcut') return { kind, key: str(o.key) };
  if (kind === 'drag') return { kind, active: bool(o.active) };
  return { kind: 'unavailable', reason: str(o.reason, 'Drag observation is unavailable') };
}

function readJobEvent(raw: unknown): JobEvent {
  const o = record(raw, 'job-event');
  const known = [
    'queued',
    'started',
    'progress',
    'finished',
    'failed',
    'cancelled',
    'idle',
  ] as const;
  const kind = known.find((candidate) => candidate === o.kind) ?? 'idle';
  const event: JobEvent = { kind };
  if (typeof o.id === 'string') event.id = o.id;
  if (typeof o.fraction === 'number') event.fraction = o.fraction;
  if (typeof o.stage === 'string') event.stage = o.stage;
  if (Array.isArray(o.outputs)) event.outputs = strings(o.outputs);
  if (typeof o.error === 'object' && o.error !== null) {
    const error = record(o.error, 'job error');
    event.error = { code: str(error.code), message: str(error.message) };
  }
  return event;
}

function readEvent<K extends keyof AppEvents>(name: K, payload: unknown): AppEvents[K] {
  switch (name) {
    case 'open-paths':
      return strings(payload) as AppEvents[K];
    case 'drag-signal':
      return readDragSignal(payload) as AppEvents[K];
    case 'job-event':
      return readJobEvent(payload) as AppEvents[K];
    case 'wheel-mode':
      return str(payload) as AppEvents[K];
    case 'wheel-shown':
    case 'wheel-hidden':
    case 'overlay-open':
    case 'capabilities-changed':
      return null as AppEvents[K];
  }
}

/* -------------------------------------------------------------------------- */
/* Commands                                                                   */
/* -------------------------------------------------------------------------- */

export async function boot(): Promise<BootInfo> {
  if (isMock()) return mock.mockBoot();
  const o = record(await invoke('boot'), 'boot');
  const backend = record(o.backend, 'boot.backend');
  const about = record(o.about, 'boot.about');
  const capabilities = record(o.capabilities, 'boot.capabilities');
  return {
    pending_paths: strings(o.pending_paths),
    settings: readSettings(o.settings),
    backend: { kind: str(backend.kind, 'unknown'), description: str(backend.description) },
    about: {
      name: str(about.name, 'ConvertMe'),
      version: str(about.version, '0.0.0'),
      options: num(about.options),
      tools: num(about.tools),
    },
    capabilities: {
      missing: records(capabilities.missing).map((tool) => ({
        key: str(tool.key),
        label: str(tool.label),
        hint: str(tool.hint),
      })),
    },
  };
}

export async function analyze(paths: string[]): Promise<AnalyzeResult> {
  if (isMock()) return mock.mockAnalyze(paths);
  const o = record(await invoke('analyze', { paths }), 'analyze');
  return {
    count: num(o.count),
    category: typeof o.category === 'string' ? o.category : null,
    format: typeof o.format === 'string' ? o.format : null,
    thumbs: readThumbs(o.thumbs),
    skipped: readSkipped(o.skipped),
  };
}

export async function wheels(category: string, format: string | null): Promise<WheelContents> {
  if (isMock()) return mock.mockWheels(category, format);
  const o = record(await invoke('wheels', { category, format }), 'wheels');
  return {
    convert: readSegments(o.convert),
    tools: readSegments(o.tools),
    actions: readSegments(o.actions),
  };
}

export async function run(paths: string[], category: string, intent: Intent): Promise<RunResult> {
  if (isMock()) return mock.mockRun(paths, category, intent);
  return readRunResult(await invoke('run', { paths, category, intent }));
}

/** Pixel size of a picture: the preview draws a thumbnail but must report real sizes. */
export async function imageSize(path: string): Promise<{ w: number; h: number } | null> {
  if (isMock()) {
    // The mock has no file to read; a phone-sized picture keeps the maths honest.
    return { w: 4032, h: 3024 };
  }
  const raw = (await invoke('image_size', { path })) as [number, number] | null;
  return Array.isArray(raw) && raw.length === 2 ? { w: raw[0], h: raw[1] } : null;
}

/**
 * The file the compress dialog will produce, encoded once at the chosen settings. The
 * backend caches the artifact, so submitting the identical job copies it.
 */
export async function compressPreview(
  path: string,
  params: Record<string, unknown>,
): Promise<CompressPreview> {
  if (isMock()) return mock.mockCompressPreview(path, params);
  return readCompressPreview(await invoke('compress_preview', { path, params }));
}

export async function toolFields(category: string, tool: string): Promise<Field[]> {
  // The mock is parsed by the same reader the app uses, so a dropped kind fails here too.
  if (isMock()) return readFields(mock.mockToolFields(category, tool));
  return readFields(await invoke('tool_fields', { category, tool }));
}

export interface CacheStats {
  bytes: number;
  files: number;
  cap_bytes: number;
}

export async function cacheStats(): Promise<CacheStats> {
  if (isMock()) return mock.mockCacheStats();
  const o = record(await invoke('cache_stats'), 'cache stats');
  return {
    bytes: typeof o.bytes === 'number' ? o.bytes : 0,
    files: typeof o.files === 'number' ? o.files : 0,
    cap_bytes: typeof o.cap_bytes === 'number' ? o.cap_bytes : 0,
  };
}

export async function clearCache(): Promise<CacheStats> {
  if (isMock()) return mock.mockClearCache();
  const o = record(await invoke('clear_cache'), 'cache stats');
  return {
    bytes: typeof o.bytes === 'number' ? o.bytes : 0,
    files: typeof o.files === 'number' ? o.files : 0,
    cap_bytes: typeof o.cap_bytes === 'number' ? o.cap_bytes : 0,
  };
}

export async function history(limit: number): Promise<HistoryRow[]> {
  if (isMock()) return mock.mockHistory(limit);
  const raw = await invoke('history', { limit });
  return records(raw).map(readHistoryRow);
}

export async function cancelJob(id: string): Promise<void> {
  if (isMock()) return;
  await invoke('cancel_job', { id });
}

export async function cancelAll(): Promise<void> {
  if (isMock()) return;
  await invoke('cancel_all');
}

export async function retry(id: string): Promise<void> {
  if (isMock()) return;
  await invoke('retry', { id });
}

/** State of the login entry on the system, not just in our settings file. */
export async function autostartStatus(): Promise<boolean> {
  if (isMock()) return false;
  return Boolean(await invoke('autostart_status'));
}

/** Creates or removes the login entry (`convertme --background`). */
export async function setAutostart(enabled: boolean): Promise<void> {
  if (isMock()) return;
  await invoke('set_autostart', { enabled });
}

export async function saveSettings(settings: Settings): Promise<void> {
  if (isMock()) return;
  await invoke('save_settings', { settings });
}

/**
 * File actions (reveal, open, copy, move, share, terminal, preview) all go
 * through the backend's single `run_action` entry point. Copy and move need a
 * destination chosen in the frontend first.
 */
export async function runAction(action: string, paths: string[], dest?: string): Promise<void> {
  if (isMock()) return;
  await invoke('run_action', { action, paths, dest: dest ?? null });
}

/** Drain paths that arrived while the window was open. */
export async function takePendingPaths(): Promise<string[]> {
  if (isMock()) return [];
  return strings(await invoke('take_pending_paths'));
}

/** Open the wheel for the current selection, from a button in the window. */
export async function previewWheel(paths: string[]): Promise<void> {
  if (isMock()) return;
  await invoke('preview_wheel', { paths });
}

export async function wheelMode(mode: string, x?: number, y?: number): Promise<void> {
  if (isMock()) return;
  await invoke('wheel_mode', { mode, x: x ?? null, y: y ?? null });
}

/** Native folder picker for the Copy/Move actions. */
export async function pickFolder(): Promise<string | null> {
  if (isMock()) return '/home/you/Documents';
  const chosen = await open({ directory: true, multiple: false });
  return typeof chosen === 'string' ? chosen : null;
}

export async function previewThumb(path: string, size: number): Promise<string> {
  if (isMock()) return mock.mockPreviewThumb(path, size);
  const raw = await invoke('preview_thumb', { path, size });
  return str(raw);
}

// --------------------------------------------------------------- dialog data

export async function mediaInfo(path: string): Promise<MediaInfo> {
  if (isMock()) return mock.mockMediaInfo(path);
  return readMediaInfo(await invoke('media_info', { path }));
}

export async function filmstrip(path: string, frames: number): Promise<string[]> {
  if (isMock()) return mock.mockFilmstrip(frames);
  const raw = await invoke('filmstrip', { path, frames });
  return strings(raw);
}

/** The images Play walks: sampled at `fps`, at most `max` stills. */
export async function videoFrames(
  path: string,
  fps: number,
  max: number,
): Promise<string[]> {
  if (isMock()) return mock.mockPlaybackFrames(max);
  const raw = await invoke('video_frames', { path, fps, max });
  return strings(raw);
}

export async function waveformData(path: string): Promise<WaveformData> {
  if (isMock()) return mock.mockWaveformData(path);
  return readWaveformData(await invoke('waveform_data', { path }));
}

export async function channelWaveforms(path: string): Promise<ChannelWaveforms> {
  if (isMock()) return mock.mockChannelWaveforms(path);
  const raw = record(await invoke('channel_waveforms', { path }), 'channels');
  return {
    left: readWaveformData(raw.left),
    right: readWaveformData(raw.right),
  };
}

export async function pdfPages(path: string): Promise<PdfPage[]> {
  if (isMock()) return mock.mockPdfPages(path);
  return records(await invoke('pdf_pages', { path })).map((o) => ({
    page: typeof o.page === 'number' ? o.page : 0,
    width: typeof o.width === 'number' ? o.width : 0,
    height: typeof o.height === 'number' ? o.height : 0,
    thumb: typeof o.thumb === 'string' ? o.thumb : null,
  }));
}

export async function measureLoudness(path: string): Promise<Loudness> {
  if (isMock()) return mock.mockLoudness(path);
  const o = record(await invoke('measure_loudness', { path }), 'loudness');
  return {
    input_i: typeof o.input_i === 'number' ? o.input_i : 0,
    input_tp: typeof o.input_tp === 'number' ? o.input_tp : 0,
    input_lra: typeof o.input_lra === 'number' ? o.input_lra : 0,
  };
}

/** A tool's result, rendered by the engine and transcoded for playback. */
export async function audioPreview(
  path: string,
  tool: string,
  params: Record<string, unknown>,
): Promise<string> {
  if (isMock()) return mock.mockAudioPreview(path, tool, params);
  return str(await invoke('audio_preview', { path, tool, params }));
}

export async function playPreview(path: string): Promise<void> {
  if (isMock()) return;
  await invoke('play_preview', { path });
}

export async function bleepPreview(
  path: string,
  params: Record<string, unknown>,
): Promise<string> {
  if (isMock()) return mock.mockAudioPreview(path, 'bleep', params);
  return str(await invoke('bleep_preview', { path, params }));
}

export async function photoPreview(
  path: string,
  params: Record<string, unknown>,
): Promise<string> {
  if (isMock()) return mock.mockPhotoPreview(path, params);
  return str(await invoke('photo_preview', { path, params }));
}

export async function collagePreview(
  paths: string[],
  params: Record<string, unknown>,
): Promise<string> {
  if (isMock()) return mock.mockCollagePreview(paths, params);
  return str(await invoke('collage_preview', { paths, params }));
}

/**
 * The cell arrangement the collage composer will use, in output pixels. The preview
 * overlays these rectangles so a drag moves the picture where the file will place it;
 * computing the layout here instead would be a second implementation of the grid.
 */
export async function collageLayout(
  paths: string[],
  params: Record<string, unknown>,
): Promise<CollageLayout> {
  if (isMock()) return mock.mockCollageLayout(paths, params);
  return readCollageLayout(await invoke('collage_layout', { paths, params }));
}

/**
 * A redacted still, produced by the real redaction engine at preview size. The dialog
 * shows it beside the editable boxes so the treatment can be inspected before saving.
 */
export async function redactPreview(
  path: string,
  params: Record<string, unknown>,
): Promise<string> {
  if (isMock()) return mock.mockRedactPreview(path, params);
  return str(await invoke('redact_preview', { path, params }));
}

export async function fileMetadata(path: string): Promise<MetaGroup[]> {
  if (isMock()) return mock.mockFileMetadata(path);
  return records(await invoke('file_metadata', { path })).map((group) => ({
    name: str(group.name),
    fields: records(group.fields).map((field) => ({
      key: str(field.key),
      label: str(field.label),
      value: str(field.value),
      editable: field.editable === true,
      removable: field.removable === true,
    })),
  }));
}

function readWaveformData(value: unknown): WaveformData {
  const o = record(value, 'waveform');
  return {
    duration_ms: typeof o.duration_ms === 'number' ? o.duration_ms : 0,
    truncated: o.truncated === true,
    peaks: records(o.peaks).map((pair) => {
      const values = Array.isArray(pair) ? pair : [];
      return [Number(values[0]) || 0, Number(values[1]) || 0] as [number, number];
    }),
  };
}

function readMediaInfo(value: unknown): MediaInfo {
  const o = record(value, 'media info');
  return {
    duration: typeof o.duration === 'number' ? o.duration : 0,
    width: typeof o.width === 'number' ? o.width : 0,
    height: typeof o.height === 'number' ? o.height : 0,
    has_audio: o.has_audio === true,
    has_video: o.has_video === true,
    format: str(o.format),
  };
}

export async function detectBoxes(path: string, faces: boolean): Promise<DetectedBox[]> {
  if (isMock()) return mock.mockDetectBoxes(path, faces);
  return readDetectedBoxes(await invoke('detect_boxes', { path, faces }));
}

export async function pickFiles(): Promise<string[]> {
  if (isMock()) return mock.mockPickFiles();
  const chosen = await open({ multiple: true, directory: false });
  if (chosen === null) return [];
  return Array.isArray(chosen) ? chosen : [chosen];
}

export async function wheelShow(x: number, y: number): Promise<void> {
  if (isMock()) return;
  await invoke('wheel_show', { x, y });
}

export async function wheelHide(): Promise<void> {
  if (isMock()) return;
  await invoke('wheel_hide');
}

export async function overlayOpen(): Promise<void> {
  if (isMock()) return;
  await invoke('overlay_open');
}

export async function overlayClose(): Promise<void> {
  if (isMock()) return;
  await invoke('overlay_close');
}

/* -------------------------------------------------------------------------- */
/* Events                                                                     */
/* -------------------------------------------------------------------------- */

/** Listen to an untyped event (perf handshakes, future additions). */
export function onAnyEvent(name: string, handler: (payload: unknown) => void): Promise<UnlistenFn> {
  if (isMock()) return Promise.resolve(noopUnlisten);
  return listen<unknown>(name, (event) => handler(event.payload));
}

/** Emit an event from the webview (used for `ui-ready` and the perf handshake). */
export async function emitToBackend(name: string, payload?: unknown): Promise<void> {
  if (isMock()) return;
  await emit(name, payload ?? null);
}

export function onEvent<K extends keyof AppEvents>(
  name: K,
  handler: (payload: AppEvents[K]) => void,
): Promise<UnlistenFn> {
  if (isMock()) {
    return Promise.resolve(mock.mockOnEvent(name, (payload) => handler(readEvent(name, payload))));
  }
  return listen<unknown>(name, (event) => handler(readEvent(name, event.payload)));
}

/**
 * File drops in the main window. Tauri maps `type` to the payload kind; the
 * position is intentionally dropped, nothing in the UI depends on it.
 */
export async function subscribeDragDrop(
  handler: (payload: DropPayload) => void,
): Promise<UnlistenFn> {
  if (isMock()) return noopUnlisten;
  try {
    return await getCurrentWebview().onDragDropEvent((event) => {
      const payload = record(event.payload, 'drag-drop');
      const kind: DropPayload['kind'] =
        payload.type === 'enter' ||
        payload.type === 'over' ||
        payload.type === 'drop' ||
        payload.type === 'leave'
          ? payload.type
          : 'over';
      handler({ kind, paths: strings(payload.paths) });
    });
  } catch {
    return noopUnlisten;
  }
}
