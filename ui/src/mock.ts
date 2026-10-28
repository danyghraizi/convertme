import { ACTIONS, FALLBACK_TARGETS, FALLBACK_TOOLS } from './catalog';
import type {
  AnalyzeResult,
  BootInfo,
  DetectedBox,
  Field,
  HistoryRow,
  JobEvent,
  RunResult,
  Segment,
  Settings,
  WheelContents,
} from './types';

/** Fixture paths used by `pick_files` and by the wheel's browser demo. */
export const MOCK_DEMO_PATHS = ['/fixtures/photo.jpg', '/fixtures/poster.png'];

const DEFAULT_SETTINGS: Settings = {
  convert_key: 'shift',
  tools_key: 'control',
  actions_key: 'alt',
  overwrite: 'never',
  history_limit: 100,
  preview_cache_bytes: 256 * 1024 * 1024,
  ocr_languages: ['eng'],
  update_check: false,
  drag_backend: 'auto',
  start_at_login: false,
  redact_fill: '#111111',
  extra_tools_dir: null,
};

const BOOT: BootInfo = {
  settings: { ...DEFAULT_SETTINGS },
  backend: {
    kind: 'mock',
    description: 'Browser preview: fixture data, no native backend attached',
  },
  about: { name: 'ConvertMe', version: '0.1.0', options: 0, tools: 0 },
  capabilities: { missing: [] },
  pending_paths: [],
};

type MockEventName =
  | 'open-paths'
  | 'drag-signal'
  | 'job-event'
  | 'wheel-mode'
  | 'wheel-shown'
  | 'wheel-hidden'
  | 'overlay-open'
  | 'capabilities-changed';

type Listener = (payload: unknown) => void;

const listeners = new Map<MockEventName, Set<Listener>>();

export function mockOnEvent(name: MockEventName, cb: (payload: unknown) => void): () => void {
  let set = listeners.get(name);
  if (!set) {
    set = new Set();
    listeners.set(name, set);
  }
  set.add(cb);
  return () => {
    set.delete(cb);
  };
}

export function mockEmitEvent(name: MockEventName, payload: unknown): void {
  const set = listeners.get(name);
  if (!set) return;
  for (const cb of set) cb(payload);
}

const EXTENSIONS: Record<string, { category: string; format: string }> = {
  png: { category: 'images', format: 'PNG' },
  jpg: { category: 'images', format: 'JPEG' },
  jpeg: { category: 'images', format: 'JPEG' },
  webp: { category: 'images', format: 'WEBP' },
  avif: { category: 'images', format: 'AVIF' },
  heic: { category: 'images', format: 'HEIC' },
  tif: { category: 'images', format: 'TIFF' },
  tiff: { category: 'images', format: 'TIFF' },
  bmp: { category: 'images', format: 'BMP' },
  gif: { category: 'images', format: 'GIF' },
  ico: { category: 'images', format: 'ICO' },
  mp4: { category: 'video', format: 'MP4' },
  mkv: { category: 'video', format: 'MKV' },
  webm: { category: 'video', format: 'WEBM' },
  mov: { category: 'video', format: 'MOV' },
  avi: { category: 'video', format: 'AVI' },
  mp3: { category: 'audio', format: 'MP3' },
  wav: { category: 'audio', format: 'WAV' },
  flac: { category: 'audio', format: 'FLAC' },
  ogg: { category: 'audio', format: 'OGG' },
  m4a: { category: 'audio', format: 'M4A' },
  pdf: { category: 'pdf', format: 'PDF' },
  docx: { category: 'documents', format: 'DOCX' },
  odt: { category: 'documents', format: 'ODT' },
  md: { category: 'documents', format: 'MD' },
  txt: { category: 'documents', format: 'TXT' },
  html: { category: 'documents', format: 'HTML' },
  epub: { category: 'epub', format: 'EPUB' },
  pptx: { category: 'slides', format: 'PPTX' },
  ppt: { category: 'slides', format: 'PPT' },
  xlsx: { category: 'spreadsheets', format: 'XLSX' },
  xls: { category: 'spreadsheets', format: 'XLS' },
  ods: { category: 'spreadsheets', format: 'ODS' },
  csv: { category: 'spreadsheets', format: 'CSV' },
  srt: { category: 'subtitles', format: 'SRT' },
  vtt: { category: 'subtitles', format: 'VTT' },
  zip: { category: 'archives', format: 'ZIP' },
  tar: { category: 'archives', format: 'TAR' },
  gz: { category: 'archives', format: 'GZ' },
  '7z': { category: 'archives', format: '7Z' },
  rar: { category: 'archives', format: 'RAR' },
};

function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function extensionOf(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf('.');
  return dot >= 0 ? name.slice(dot + 1).toLowerCase() : '';
}

function svgDataUrl(title: string, width: number, height: number): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><rect width="100%" height="100%" fill="#e8e8e6"/><line x1="0" y1="0" x2="${width}" y2="${height}" stroke="#c6c6c2" stroke-width="2"/><line x1="${width}" y1="0" x2="0" y2="${height}" stroke="#c6c6c2" stroke-width="2"/><text x="${Math.round(width / 2)}" y="${Math.round(height / 2)}" font-family="sans-serif" font-size="${Math.round(Math.min(width, height) / 12)}" fill="#5a5a56" text-anchor="middle">${title}</text></svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

export function mockBoot(): BootInfo {
  return structuredClone(BOOT);
}

export function mockAnalyze(paths: string[]): AnalyzeResult {
  const thumbs = [];
  const skipped = [];
  const seen = new Set<string>();
  let category: string | null = null;
  let format: string | null = null;
  let mixed = false;

  for (const path of paths) {
    const ext = extensionOf(path);
    const known = EXTENSIONS[ext];
    if (!known) {
      skipped.push({ path, message: `Unsupported file type ".${ext || '?'}"` });
      continue;
    }
    if (category === null) {
      category = known.category;
      format = known.format;
    } else if (category !== known.category) {
      mixed = true;
    }
    if (!seen.has(path)) {
      seen.add(path);
      thumbs.push({ path, thumb: svgDataUrl(baseName(path), 160, 120) });
    }
  }

  return {
    count: thumbs.length,
    category: mixed ? null : category,
    format: mixed ? null : format,
    thumbs,
    skipped,
  };
}

export function mockWheels(category: string, format: string | null): WheelContents {
  const key = category.toLowerCase();
  const targets = FALLBACK_TARGETS[key] ?? [];
  const convert = targets
    .filter((target) => !format || target.label.toUpperCase() !== format.toUpperCase())
    .map((target) => ({ ...target }));
  const tools = (FALLBACK_TOOLS[key] ?? []).map((tool) => ({ ...tool }));
  const actions: Segment[] = ACTIONS.map((action) => ({ ...action }));
  return { convert, tools, actions };
}

const TOOL_FIELDS: Record<string, Field[]> = {
  compress: [
    {
      key: 'target_kb',
      label: 'Target size',
      kind: 'number',
      default: 800,
      min: 20,
      max: 102400,
      step: 20,
      unit: 'KB',
    },
    { key: 'min_quality', label: 'Lowest quality', kind: 'range', default: 45, min: 10, max: 90, step: 5 },
  ],
  trim: [
    { key: 'start', label: 'Start', kind: 'number', default: 0, min: 0, step: 0.1, unit: 's' },
    { key: 'end', label: 'End', kind: 'number', default: 30, min: 0, step: 0.1, unit: 's' },
  ],
  crop: [
    {
      key: 'aspect',
      label: 'Aspect ratio',
      kind: 'select',
      default: 'free',
      options: ['free', '1:1', '4:3', '3:2', '16:9', '9:16'],
    },
    { key: 'area', label: 'Crop area', kind: 'boxes', default: [] },
  ],
  redact: [
    {
      key: 'mode',
      label: 'What to cover',
      kind: 'select',
      default: 'faces',
      options: ['faces', 'text', 'faces and text', 'manual'],
    },
    { key: 'fill', label: 'Fill colour', kind: 'color', default: '#111111' },
    { key: 'regions', label: 'Regions', kind: 'boxes', default: [] },
  ],
  organize: [
    { key: 'pages', label: 'Page order', kind: 'order', default: [1, 2, 3, 4] },
    { key: 'rotate', label: 'Rotate all', kind: 'number', default: 0, min: -270, max: 270, step: 90, unit: '\u00b0' },
  ],
  metadata: [
    { key: 'title', label: 'Title', kind: 'text', default: '' },
    { key: 'author', label: 'Author', kind: 'text', default: '' },
    { key: 'strip_other', label: 'Remove all other tags', kind: 'checkbox', default: false },
  ],
  speed: [
    { key: 'rate', label: 'Speed', kind: 'range', default: 1, min: 0.25, max: 4, step: 0.25, unit: '\u00d7' },
    { key: 'keep_pitch', label: 'Keep pitch', kind: 'checkbox', default: true },
  ],
  channels: [
    {
      key: 'layout',
      label: 'Channel layout',
      kind: 'select',
      default: 'stereo',
      options: ['mono', 'stereo'],
    },
  ],
  shift: [
    { key: 'offset', label: 'Shift by', kind: 'number', default: 0, step: 0.1, unit: 's' },
  ],
  split: [
    { key: 'parts', label: 'Number of parts', kind: 'number', default: 2, min: 2, max: 50, step: 1 },
  ],
};

export function mockToolFields(_category: string, tool: string): Field[] {
  const fields = TOOL_FIELDS[tool] ?? [];
  return structuredClone(fields);
}

let historySeq = 1000;
const historyRows: HistoryRow[] = [
  {
    id: 'job-0994',
    input: '/fixtures/photo.jpg',
    output: '/fixtures/photo.webp',
    status: 'done',
    started: Date.now() - 1000 * 60 * 12,
    ended: Date.now() - 1000 * 60 * 11,
    intent: 'convert to WEBP',
  },
  {
    id: 'job-0993',
    input: '/fixtures/notes.pdf',
    output: '/fixtures/notes.docx',
    status: 'done',
    started: Date.now() - 1000 * 60 * 40,
    ended: Date.now() - 1000 * 60 * 38,
    intent: 'convert to DOCX',
  },
  {
    id: 'job-0992',
    input: '/fixtures/broken.pdf',
    output: null,
    status: 'failed',
    started: Date.now() - 1000 * 60 * 55,
    ended: Date.now() - 1000 * 60 * 54,
    error_code: 'engine_failed',
    error_tail: 'PDF header not found',
    intent: 'convert to PNG',
  },
];

export function mockHistory(limit: number): HistoryRow[] {
  return historyRows.slice(0, Math.max(0, limit)).map((row) => ({ ...row }));
}

let jobSeq = 0;
type MockTimer = ReturnType<typeof setTimeout>;
const activeTimers = new Set<MockTimer>();

function later(fn: () => void, ms: number): void {
  const handle = setTimeout(() => {
    activeTimers.delete(handle);
    fn();
  }, ms);
  activeTimers.add(handle);
}

export function mockRun(
  paths: string[],
  category: string,
  intent: { kind: 'convert'; target: string } | { kind: 'tool'; tool: string; params: object },
): RunResult {
  const valid = paths.filter((path) => path.trim().length > 0);
  const jobs = valid.map((path) => {
    jobSeq += 1;
    const id = `job-${String(historySeq + jobSeq).padStart(4, '0')}`;
    const label =
      intent.kind === 'convert'
        ? `convert to ${intent.target.toUpperCase()}`
        : `tool ${intent.tool}`;
    const row: HistoryRow = {
      id,
      input: path,
      output: null,
      status: 'queued',
      started: Date.now(),
      ended: null,
      intent: label,
    };
    historyRows.unshift(row);
    if (historyRows.length > 200) historyRows.pop();

    const emit = (event: JobEvent): void => mockEmitEvent('job-event', event);
    emit({ kind: 'queued', id });
    later(() => {
      row.status = 'running';
      emit({ kind: 'started', id, stage: category });
    }, 180);
    later(() => emit({ kind: 'progress', id, fraction: 0.45, stage: 'working' }), 700);
    later(() => {
      emit({ kind: 'progress', id, fraction: 0.85, stage: 'publishing' });
    }, 1100);
    later(() => {
      const ext = intent.kind === 'convert' ? intent.target.toLowerCase() : 'out';
      const stem = baseName(path).replace(/\.[^.]+$/, '');
      const output = `/fixtures/${stem}-converted.${ext}`;
      row.status = 'done';
      row.output = output;
      row.ended = Date.now();
      emit({ kind: 'finished', id, outputs: [output] });
      emit({ kind: 'idle' });
    }, 1700);

    return id;
  });

  return { jobs, skipped: [] };
}

export function mockPreviewThumb(path: string, size: number): string {
  return svgDataUrl(baseName(path), size, Math.round((size * 3) / 4));
}

export function mockDetectBoxes(path: string, faces: boolean): DetectedBox[] {
  const seed = path.length % 3;
  const boxes: DetectedBox[] = [];
  if (faces) {
    boxes.push({
      rect: { x: 0.32 + seed * 0.02, y: 0.18, w: 0.22, h: 0.3 },
      kind: 'face',
      detail: 'face 0.94',
    });
    boxes.push({ rect: { x: 0.62, y: 0.24, w: 0.18, h: 0.24 }, kind: 'face', detail: 'face 0.88' });
  } else {
    boxes.push({ rect: { x: 0.1, y: 0.62, w: 0.5, h: 0.08 }, kind: 'text', detail: 'line' });
    boxes.push({ rect: { x: 0.1, y: 0.76, w: 0.72, h: 0.08 }, kind: 'text', detail: 'line' });
  }
  return boxes;
}

export function mockPickFiles(): string[] {
  return [...MOCK_DEMO_PATHS];
}
