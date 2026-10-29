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
  'number' | 'text' | 'range' | 'color' | 'select' | 'checkbox' | 'boxes' | 'order';

export type FieldValue = string | number | boolean | number[] | string[] | Box[];

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
  /** A form control (a mode switch), never sent to the backend. */
  control?: boolean;
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
