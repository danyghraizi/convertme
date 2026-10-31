import { ACTIONS, FALLBACK_TARGETS, FALLBACK_TOOLS } from './catalog';
import generatedFields from './fields.generated.json';
import { readFields } from './field-parse';
import { baseName, extensionOf, isLosslessImage } from './format';
import type {
  AnalyzeResult,
  BootInfo,
  CompressPreview,
  DetectedBox,
  Field,
  HistoryRow,
  JobEvent,
  Loudness,
  MediaInfo,
  MetaGroup,
  PdfPage,
  RunResult,
  Segment,
  Settings,
  WaveformData,
  ChannelWaveforms,
  WheelContents,
} from './types';

/** Fixture paths used by `pick_files` and by the wheel's browser demo. */
export const MOCK_DEMO_PATHS = ['/fixtures/photo.jpg', '/fixtures/poster.png'];

/**
 * Files the picker returns for a category, selected with `?mock-files=<category>`.
 * They do not exist on disk; the mock only needs the extensions to pick a category.
 */
const DEMO_FILES: Record<string, string[]> = {
  images: MOCK_DEMO_PATHS,
  'images-lossless': ['/fixtures/poster.png'],
  video: ['/fixtures/clip.mp4'],
  audio: ['/fixtures/speech.mp3'],
  pdf: ['/fixtures/report-5-pages.pdf'],
  documents: ['/fixtures/report.docx'],
  slides: ['/fixtures/deck.pptx'],
  spreadsheets: ['/fixtures/sales.csv'],
  epub: ['/fixtures/report.epub'],
  subtitles: ['/fixtures/cues.srt'],
  archives: ['/fixtures/project.zip'],
};

const DEFAULT_SETTINGS: Settings = {
  convert_key: 'shift',
  tools_key: 'control',
  actions_key: 'alt',
  overwrite: 'never',
  history_limit: 100,
  preview_cache_bytes: 128 * 1024 * 1024,
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
  svg: { category: 'images', format: 'SVG' },
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
      thumbs.push({
        path,
        thumb: svgDataUrl(baseName(path), 160, 120),
        size: 1_400_000 + path.length * 900,
      });
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

/**
 * The tool's fields, exactly as the backend declares them.
 *
 * `ui/src/fields.generated.json` is written by `scripts/gen-ui-fields.py` from
 * `convertme probe --fields`, and a test fails when it is stale. The mock therefore
 * cannot show a control the real dialog does not have — a hand-written copy did.
 */
export function mockToolFields(category: string, tool: string): Field[] {
  const byCategory = (generatedFields as Record<string, Record<string, unknown>>)[category];
  const fields = byCategory?.[tool];
  if (!Array.isArray(fields)) return [];
  return readFields(fields);
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

/**
 * The mock cannot encode, so it derives the same numbers the dialog displays from the
 * fields. The real surface gets the size from the backend's actual encode.
 */
export function mockCompressPreview(
  path: string,
  params: Record<string, unknown>,
): CompressPreview {
  const source = 1_400_000 + path.length * 900;
  const scale = Number(params.scale ?? 100) || 100;
  const quality = Number(params.quality ?? 80) || 80;
  const target = Number(params.target_bytes ?? 0) || 0;
  const width = Math.max(1, Math.round((4032 * scale) / 100));
  const height = Math.max(1, Math.round((3024 * scale) / 100));
  const factor = isLosslessImage(path)
    ? Math.min(1, (scale / 100) ** 2)
    : Math.min(0.95, Math.max(0.02, (quality / 100) ** 2 * (scale / 100) ** 2));
  // A real PNG has a floor (the 16-bit poster cannot reach 1 KB); without one the mock
  // would claim every target is met and the unreachable-target note would never render.
  const floor = isLosslessImage(path) ? 9_000 : 1_024;
  const bytes =
    target > 0
      ? Math.max(Math.min(Math.round(target), source), Math.min(floor, source))
      : Math.round(source * factor);
  return {
    path: svgDataUrl(baseName(path), 512, Math.max(1, Math.round((512 * height) / width))),
    bytes,
    source_bytes: source,
    width,
    height,
  };
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
    boxes.push({
      rect: { x: 0.62, y: 0.24, w: 0.18, h: 0.24 },
      kind: 'face',
      detail: 'face 0.88',
    });
  } else {
    boxes.push({
      rect: { x: 0.1, y: 0.62, w: 0.5, h: 0.08 },
      kind: 'text',
      detail: 'line',
    });
    boxes.push({
      rect: { x: 0.1, y: 0.76, w: 0.72, h: 0.08 },
      kind: 'text',
      detail: 'line',
    });
  }
  return boxes;
}

export function mockPickFiles(): string[] {
  const category =
    typeof location === 'undefined'
      ? null
      : new URLSearchParams(location.search).get('mock-files');
  return [...(DEMO_FILES[category ?? 'images'] ?? MOCK_DEMO_PATHS)];
}

// ------------------------------------------------------------ dialog data

/** A deterministic pseudo-random number from a string, so a mock never flickers. */
function hashSeed(value: string): number {
  let hash = 2166136261;
  for (let i = 0; i < value.length; i += 1) {
    hash ^= value.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return Math.abs(hash);
}

/**
 * The mock's media facts. A video shows 10 s and the frame size the fixture name
 * implies; audio shows a track length; an image shows its pixels. The real command
 * reads them from the file, so nothing here may be the only source of a number the
 * layout depends on.
 */
export function mockMediaInfo(path: string): MediaInfo {
  const ext = extensionOf(path);
  const video = ['mp4', 'mov', 'mkv', 'webm', 'avi', 'wmv', 'm4v'];
  const audio = ['mp3', 'm4a', 'wav', 'flac', 'ogg', 'oga', 'opus', 'aiff', 'aif', 'wma'];
  if (video.includes(ext)) {
    const portrait = path.includes('portrait');
    return {
      duration: 10,
      width: portrait ? 1080 : 1280,
      height: portrait ? 1920 : 720,
      has_audio: !path.includes('silent'),
      has_video: true,
      format: ext,
    };
  }
  if (audio.includes(ext)) {
    return {
      duration: path.includes('30s') ? 30 : 14.1,
      width: 0,
      height: 0,
      has_audio: true,
      has_video: false,
      format: ext,
    };
  }
  const landscape = !path.includes('portrait') && !path.includes('poster');
  return {
    duration: 0,
    width: landscape ? 4000 : 2000,
    height: landscape ? 3000 : 3000,
    has_audio: false,
    has_video: false,
    format: ext,
  };
}

export function mockFilmstrip(frames: number): string[] {
  const count = Math.max(1, Math.min(12, frames));
  return Array.from({ length: count }, (_, index) =>
    svgDataUrl(`${index + 1}`, 320, 180),
  );
}

/** A playback sequence for the timeline: enough stills to see Play move. */
export function mockPlaybackFrames(max: number): string[] {
  const count = Math.max(1, Math.min(36, max));
  return Array.from({ length: count }, (_, index) =>
    svgDataUrl(`${index + 1}`, 320, 180),
  );
}

/** A waveform that looks like speech: loud in the middle, quiet at the ends. */
export function mockWaveformData(path: string): WaveformData {
  const info = mockMediaInfo(path);
  const buckets = 400;
  const peaks: [number, number][] = [];
  for (let i = 0; i < buckets; i += 1) {
    const at = i / buckets;
    const envelope = Math.sin(Math.PI * at) ** 0.6;
    const wiggle = 0.35 + 0.65 * Math.abs(Math.sin(i / 7 + hashSeed(path) % 7));
    const amplitude = envelope * wiggle;
    peaks.push([-amplitude, amplitude]);
  }
  return { duration_ms: Math.round(info.duration * 1000), truncated: false, peaks };
}

export function mockChannelWaveforms(path: string): ChannelWaveforms {
  const left = mockWaveformData(path);
  const right = mockWaveformData(`${path}-right`);
  return {
    left,
    right: {
      ...right,
      peaks: right.peaks.map(([lo, hi]) => [lo * 0.6, hi * 0.6] as [number, number]),
    },
  };
}

export function mockPdfPages(path: string): PdfPage[] {
  const pages = path.includes('scanned') ? 3 : 5;
  return Array.from({ length: pages }, (_, index) => ({
    page: index + 1,
    width: 595,
    height: 842,
    thumb: svgDataUrl(`${index + 1}`, 149, 211),
  }));
}

export function mockLoudness(path: string): Loudness {
  const quiet = path.includes('quiet') || path.includes('speech');
  return {
    input_i: quiet ? -23.4 : -14.2,
    input_tp: quiet ? -6.1 : -0.8,
    input_lra: 7.4,
  };
}

export function mockAudioPreview(
  path: string,
  tool: string,
  params: Record<string, unknown>,
): string {
  void tool;
  void params;
  void path;
  // Not playable in the mock, but a URL the dialog can point at, which is what the
  // layout check needs.
  return svgDataUrl('preview', 320, 80);
}

export function mockPhotoPreview(path: string, params: Record<string, unknown>): string {
  const exposure = Number(params.exposure ?? 0);
  const label = exposure === 0 ? baseName(path) : `${exposure > 0 ? '+' : ''}${exposure} EV`;
  return svgDataUrl(label, 1024, 768);
}

export function mockCollagePreview(paths: string[], params: Record<string, unknown>): string {
  const count = Math.max(1, paths.length);
  const style = String(params.style ?? 'grid');
  return svgDataUrl(`collage · ${style} · ${count}`, 512, 512);
}

export function mockFileMetadata(path: string): MetaGroup[] {
  const ext = extensionOf(path);
  const image = ['jpg', 'jpeg', 'png', 'webp', 'tif', 'tiff', 'heic', 'avif'];
  const audio = ['mp3', 'm4a', 'wav', 'flac', 'ogg', 'opus', 'aiff', 'wma'];
  const group = image.includes(ext)
    ? {
        name: 'Image',
        fields: ['Title', 'Description', 'Artist', 'Copyright'].map((tag) => ({
          key: tag,
          label: tag,
          value: '',
          editable: true,
          removable: true,
        })),
      }
    : audio.includes(ext)
      ? {
          name: 'Audio',
          fields: ['Title', 'Artist', 'Album', 'Year'].map((tag) => ({
            key: tag,
            label: tag,
            value: tag === 'Title' ? baseName(path) : '',
            editable: true,
            removable: true,
          })),
        }
      : {
          name: 'Document',
          fields: ['Title', 'Author', 'Subject', 'Keywords', 'Creator', 'Producer'].map((tag) => ({
            key: tag,
            label: tag,
            value: '',
            editable: true,
            removable: true,
          })),
        };
  return [
    {
      name: 'File',
      fields: [
        { key: 'Name', label: 'Name', value: baseName(path), editable: false, removable: false },
        { key: 'Size', label: 'Size', value: '2.4 MB', editable: false, removable: false },
        {
          key: 'Format',
          label: 'Format',
          value: ext.toUpperCase(),
          editable: false,
          removable: false,
        },
      ],
    },
    group,
  ];
}

/** The mock's cache: a plausible number that resetting takes to zero. */
let mockCacheBytes = 41 * 1024 * 1024;

export function mockCacheStats(): { bytes: number; files: number; cap_bytes: number } {
  return {
    bytes: mockCacheBytes,
    files: mockCacheBytes > 0 ? 37 : 0,
    cap_bytes: 128 * 1024 * 1024,
  };
}

export function mockClearCache(): { bytes: number; files: number; cap_bytes: number } {
  mockCacheBytes = 0;
  return mockCacheStats();
}
