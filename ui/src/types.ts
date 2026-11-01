export type WheelKey = 'shift' | 'control' | 'alt' | 'meta';

export type OverwritePolicy = 'never' | 'ask' | 'replace';

export type DragBackend = 'auto' | 'windows' | 'x11' | 'wayland';

export interface Settings {
  convert_key: WheelKey;
  tools_key: WheelKey;
  actions_key: WheelKey;
  overwrite: OverwritePolicy;
  history_limit: number;
  preview_cache_bytes: number;
  ocr_languages: string[];
  update_check: boolean;
  drag_backend: string;
  start_at_login: boolean;
  redact_fill: string;
  /** Optional directory searched before the bundled tools. */
  extra_tools_dir: string | null;
}

export interface BackendInfo {
  kind: string;
  description: string;
}

export interface AboutInfo {
  name: string;
  version: string;
  options: number;
  tools: number;
}

export interface MissingTool {
  key: string;
  label: string;
  hint: string;
}

export interface Capabilities {
  missing: MissingTool[];
}

export interface BootInfo {
  settings: Settings;
  backend: BackendInfo;
  about: AboutInfo;
  capabilities: Capabilities;
  /** Files delivered on the command line or forwarded by a second launch. */
  pending_paths: string[];
}

export interface Segment {
  id: string;
  label: string;
  blurb?: string;
  /** Dialog copy from the tool table: sheet/rail heading and primary button. */
  title?: string;
  action?: string;
  /** The rail's check lines, verbatim. */
  benefits?: string[];
  /** Replaces the category label above the rail title ("Any file"). */
  eyebrow?: string;
  disabled?: boolean;
  hint?: string;
}

export interface WheelContents {
  convert: Segment[];
  tools: Segment[];
  actions: Segment[];
}

export type WheelGroup = keyof WheelContents;

export interface Thumb {
  path: string;
  thumb: string;
  /** Bytes on disk. */
  size: number;
}

export interface SkippedFile {
  path: string;
  message: string;
}

export interface AnalyzeResult {
  count: number;
  category: string | null;
  format: string | null;
  thumbs: Thumb[];
  skipped: SkippedFile[];
}

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type FieldKind =
  | 'number'
  | 'text'
  | 'range'
  | 'color'
  | 'gradient'
  | 'segmented'
  | 'file'
  | 'select'
  | 'checkbox'
  | 'boxes'
  | 'order'
  | 'shapes'
  | 'pages'
  | 'files'
  | 'metadata';

/**
 * One drawn annotation. Coordinates are fractions of the image, so the same shape
 * survives a preview rendered at any size.
 */
export interface Shape {
  kind: 'rect' | 'arrow' | 'ellipse' | 'line' | 'highlight' | 'number' | 'text';
  x: number;
  y: number;
  w: number;
  h: number;
  /** Freehand and polyline points, normalised like `x`/`y`. */
  points?: [number, number][];
  text?: string;
  color?: string;
  /** Stroke width, or point size for text. */
  size?: number;
}

/** The page organizer's value: which source pages, in which order, at what rotation. */
export interface PagesValue {
  order: number[];
  rotate: number[];
}

export interface MetaFields {
  set: Record<string, string>;
  remove: string[];
  strip: boolean;
}

export type FieldValue =
  | string
  | number
  | boolean
  | number[]
  | string[]
  | Box[]
  | Shape[]
  | PagesValue
  | MetaFields;

export interface Field {
  key: string;
  label: string;
  kind: FieldKind;
  default: FieldValue;
  min?: number;
  max?: number;
  step?: number;
  unit?: string;
  options?: string[];
  /** Presentation for `options` when the stored values are not readable. */
  option_labels?: string[];
  /** Render this field only while another field holds a given value. */
  show_if?: { key: string; equals: string };
  /** Section heading when the controls are grouped. */
  group?: string;
  /** One line of help under the control. */
  help?: string;
  /** A form control (a mode switch), never sent to the backend. */
  control?: boolean;
  /**
   * Owned by a dedicated editor: the value is still submitted, but no generic
   * control is drawn for it. The crop surface, the page grid and the timelines
   * set it, so the dialog does not show a second, unused input for the same value.
   */
  hidden?: boolean;
}

export interface ConvertIntent {
  kind: 'convert';
  target: string;
}

export interface ToolIntent {
  kind: 'tool';
  tool: string;
  params: Record<string, unknown>;
}

export type Intent = ConvertIntent | ToolIntent;

export interface RunResult {
  jobs: string[];
  skipped: SkippedFile[];
}

export type JobStatus = 'queued' | 'running' | 'done' | 'failed' | 'cancelled' | 'interrupted';

export interface HistoryRow {
  id: string;
  input: string;
  output: string | null;
  status: JobStatus;
  started: number | null;
  ended: number | null;
  error_code?: string;
  error_tail?: string;
  intent: string;
}

export interface DetectedBox {
  rect: Box;
  kind: string;
  detail: string;
}

/** The encoded file the compress dialog shows before the user commits. */
export interface CompressPreview {
  path: string;
  bytes: number;
  source_bytes: number;
  width: number;
  height: number;
}

/** One placed picture of a collage, in output pixels. */
export interface CollageCellLayout {
  /** Index into the ordered inputs. */
  input: number;
  row: number;
  col: number;
  row_span: number;
  col_span: number;
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The arrangement the engine will write: the preview draws its hit areas from it. */
export interface CollageLayout {
  width: number;
  height: number;
  cols: number;
  rows: number;
  spacing: number;
  cells: CollageCellLayout[];
}

export type DragSignal =
  | { kind: 'modifier'; key: string; active: boolean }
  | { kind: 'shortcut'; key: string }
  | { kind: 'drag'; active: boolean }
  | { kind: 'unavailable'; reason: string };

export type JobEventKind =
  'queued' | 'started' | 'progress' | 'finished' | 'failed' | 'cancelled' | 'idle';

export interface JobError {
  code: string;
  message: string;
}

export interface JobEvent {
  kind: JobEventKind;
  id?: string;
  fraction?: number;
  stage?: string;
  outputs?: string[];
  error?: JobError;
}

export interface ProgressState {
  status: 'queued' | 'running' | 'done' | 'failed' | 'cancelled';
  fraction: number;
  stage?: string;
}

/** What the timeline and the crop surface need before they can draw. */
export interface MediaInfo {
  duration: number;
  width: number;
  height: number;
  has_audio: boolean;
  has_video: boolean;
  format: string;
}

export interface WaveformData {
  duration_ms: number;
  truncated: boolean;
  /** Per-bucket min/max, in -1.0..1.0. */
  peaks: [number, number][];
}

export interface ChannelWaveforms {
  left: WaveformData;
  right: WaveformData;
}

export interface PdfPage {
  page: number;
  /** PostScript points, from the page's own MediaBox. */
  width: number;
  height: number;
  /** Rendered thumbnail; `null` past the first hundred pages. */
  thumb: string | null;
}

export interface Loudness {
  input_i: number;
  input_tp: number;
  input_lra: number;
}

export interface MetaFieldRow {
  key: string;
  label: string;
  value: string;
  /** The writer can set this tag. */
  editable: boolean;
  /** The writer can delete this tag alone, so the row offers a minus. */
  removable: boolean;
}

export interface MetaGroup {
  name: string;
  fields: MetaFieldRow[];
}
