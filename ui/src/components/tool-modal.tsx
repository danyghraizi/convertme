import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
import { createStore } from 'solid-js/store';
import * as ipc from '../ipc';
import BackgroundPreview from './background-preview';
import CollagePreview from './collage-preview';
import CompressPreview from './compress-preview';
import CropEditor from './crop-editor';
import PhotoPreview from './photo-preview';
import FieldEditor, { BoxesEditor } from './field-editor';
import AnnotateEditor from './annotate-editor';
import VideoEditor from './video-editor';
import AudioEditor from './audio-editor';
import { collectParams, initialValue, visibleIn } from '../fields';
import { formatBytes, isLosslessSelection, isPngSelection } from '../format';
import type { Box, Field, FieldValue, Segment } from '../types';

interface FillPreset {
  label: string;
  mode: 'colour' | 'gradient';
  color?: string;
  gradient?: string;
  css: string;
}

/** Ready-made fills, so a background does not start from a hex value. */
const SOLID_PRESETS: FillPreset[] = [
  { label: 'Ink', mode: 'colour', color: '#101418', css: '#101418' },
  { label: 'Paper', mode: 'colour', color: '#ffffff', css: '#ffffff' },
  { label: 'Amber', mode: 'colour', color: '#f6c26b', css: '#f6c26b' },
  { label: 'Ember', mode: 'colour', color: '#e4572e', css: '#e4572e' },
];

const GRADIENT_PRESETS: FillPreset[] = [
  {
    label: 'Warm sunset',
    mode: 'gradient',
    gradient: 'linear|90|#f6c26b@0,#e4572e@100',
    css: 'linear-gradient(90deg,#f6c26b,#e4572e)',
  },
  {
    label: 'Dusk',
    mode: 'gradient',
    gradient: 'linear|135|#0b1020@0,#4b5563@35,#fbbf24@100',
    css: 'linear-gradient(135deg,#0b1020,#4b5563 35%,#fbbf24)',
  },
  {
    label: 'Mint',
    mode: 'gradient',
    gradient: 'radial||#7ee787@0,#0f2417@100',
    css: 'radial-gradient(circle,#7ee787,#0f2417)',
  },
  {
    label: 'Sea',
    mode: 'gradient',
    gradient: 'linear|0|#2b6cb0@0,#63b3ed@100',
    css: 'linear-gradient(0deg,#2b6cb0,#63b3ed)',
  },
];


interface ToolModalProps {
  tool: Segment;
  fields: Field[];
  /** The category the tool runs in: the same id drives different fields per category. */
  category: string;
  /** The whole selection: the settings apply to all of them, the preview shows one. */
  files?: { path: string; label: string }[];
  /** Shown above the fields: the file the tool will act on, and its size. */
  fileLabel?: string | null;
  fileSize?: number | null;
  previewPath: string | null;
  onClose: () => void;
  /** One entry per file: settings can differ unless "same for every file" is ticked. */
  onSubmit: (entries: { path: string; params: Record<string, unknown> }[]) => Promise<void>;
}

/**
 * The primary button's text. The tool table writes `Save {n} Frame(s)`; a count of
 * one reads "Frame", everything else "Frames", so the button never says "1 Frames".
 */
export function actionLabel(action: string | undefined, count: number): string {
  const text = action ?? 'Run';
  if (!text.includes('{n}')) return text;
  return text
    .replace('{n}', String(count))
    .replace('Frame(s)', count === 1 ? 'Frame' : 'Frames');
}

/** Video tools whose dialog draws a timeline, crop canvas or redaction surface. */
const VIDEO_SURFACES = ['trim', 'crop', 'speed', 'split', 'snapshot', 'redact'];
/** Audio tools whose dialog draws the waveform, channels or a visualizer still. */
const AUDIO_SURFACES = ['trim', 'normalize', 'channels', 'visualize', 'bleep'];

export default function ToolModal(props: ToolModalProps) {
  const seed: Record<string, FieldValue> = {};
  for (const field of props.fields) seed[field.key] = initialValue(field);
  const [values, setValues] = createStore<Record<string, FieldValue>>(seed);
  const [submitting, setSubmitting] = createSignal(false);
  // Which file the preview shows; the settings apply to every selected file.
  const [shown, setShown] = createSignal(0);
  const [shared, setShared] = createSignal(false);
  // Settings per file, so switching the preview does not silently reuse the last ones.
  const perFile = new Map<number, Record<string, FieldValue>>();

  const fileList = (): { path: string; label: string }[] => props.files ?? [];
  /** The file the dialog is currently showing: the one its editors read. */
  const shownPath = (): string | null =>
    fileList()[shown()]?.path ?? props.previewPath;

  function seedDefaults(): void {
    for (const field of props.fields) setValues(field.key, initialValue(field));
  }

  function keep(valuesForFile: Record<string, FieldValue> | undefined): void {
    seedDefaults();
    if (valuesForFile) {
      for (const [key, value] of Object.entries(valuesForFile)) {
        setValues(key, value);
      }
    }
  }

  function go(delta: number): void {
    const count = fileList().length;
    if (count <= 1) return;
    const next = (shown() + delta + count) % count;
    if (!shared()) {
      // Store the parameters, not the whole store: `mode` is a control and the backend
      // rejects it ("unknown parameter `mode`").
      perFile.set(shown(), collected() as Record<string, FieldValue>);
      keep(perFile.get(next));
    }
    setShown(next);
  }
  const [error, setError] = createSignal('');
  /** How many items a counted action names ("Save 2 Frames"). */
  const [actionCount, setActionCount] = createSignal(0);
  /** Parameters a dedicated editor owns (`times`, `sections`). */
  const [extras, setExtras] = createSignal<Record<string, unknown>>({});
  /** False while the surface says the action has nothing to do yet. */
  const [editorReady, setEditorReady] = createSignal(true);
  /** Bumped by Reset so an editor's own marks (split points, frames) clear too. */
  const [resetToken, setResetToken] = createSignal(0);
  const hasSurface = (): boolean =>
    (props.category === 'video' && VIDEO_SURFACES.includes(props.tool.id)) ||
    (props.category === 'audio' && AUDIO_SURFACES.includes(props.tool.id));
  /**
   * Image tools whose preview must stay in view while their settings scroll: the
   * preview *is* the thing being changed, so scrolling it away is the bug.
   */
  const pinnedSurface = (): boolean =>
    props.category === 'images' &&
    ['compress', 'crop', 'redact', 'annotate', 'add-background', 'edit-photo', 'collage'].includes(
      props.tool.id,
    );
  /** Source pixels of the preview, for the crop dialog's W/H inputs. */
  let dialog: HTMLDivElement | undefined;
  let sheet: HTMLFormElement | undefined;
  /** How far the head has been dragged from the centred position. */
  const offset = { x: 0, y: 0 };

  /**
   * The sheet is parented to the window, not to a fixed spot: dragging its head moves
   * it, and it never leaves the window far enough to become unreachable.
   */
  function startMove(event: PointerEvent): void {
    if (!sheet) return;
    const target = event.target as HTMLElement;
    if (target.closest('button, input, select, textarea, a')) return;
    const head = event.currentTarget as HTMLElement;
    head.setPointerCapture(event.pointerId);
    const start = { x: event.clientX, y: event.clientY, dx: offset.x, dy: offset.y };
    const edgeX = Math.max(0, window.innerWidth / 2 - 120);
    const edgeY = Math.max(0, window.innerHeight / 2 - 80);
    const move = (e: PointerEvent): void => {
      offset.x = Math.max(-edgeX, Math.min(edgeX, start.dx + e.clientX - start.x));
      offset.y = Math.max(-edgeY, Math.min(edgeY, start.dy + e.clientY - start.y));
      sheet?.style.setProperty('--sheet-dx', `${offset.x}px`);
      sheet?.style.setProperty('--sheet-dy', `${offset.y}px`);
    };
    const up = (): void => {
      head.removeEventListener('pointermove', move);
      head.removeEventListener('pointerup', up);
      head.removeEventListener('pointercancel', up);
    };
    head.addEventListener('pointermove', move);
    head.addEventListener('pointerup', up);
    head.addEventListener('pointercancel', up);
  }

  function centre(): void {
    offset.x = 0;
    offset.y = 0;
    sheet?.style.removeProperty('--sheet-dx');
    sheet?.style.removeProperty('--sheet-dy');
  }

  /**
   * The corner, the right edge and the bottom edge all resize the sheet. The old
   * transparent corner grip was the only affordance and its height stopped at
   * 780 px, which read as "it will not resize vertically".
   */
  function beginResize(event: PointerEvent, axes: 'e' | 's' | 'se'): void {
    const target = event.currentTarget as HTMLElement;
    target.setPointerCapture(event.pointerId);
    const rect = sheet?.getBoundingClientRect();
    const start = {
      x: event.clientX,
      y: event.clientY,
      w: rect?.width ?? 600,
      h: rect?.height ?? 620,
    };
    const move = (e: PointerEvent): void => {
      const maxWidth = Math.min(window.innerWidth * 0.96, 920);
      const maxHeight = window.innerHeight * 0.94;
      if (axes.includes('e')) {
        const width = Math.min(maxWidth, Math.max(420, start.w + (e.clientX - start.x)));
        sheet?.style.setProperty('--modal-w', `${Math.round(width)}px`);
      }
      if (axes.includes('s')) {
        const height = Math.min(maxHeight, Math.max(260, start.h + (e.clientY - start.y)));
        sheet?.style.setProperty('--modal-h', `${Math.round(height)}px`);
      }
    };
    const up = (): void => {
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', up);
      target.removeEventListener('pointercancel', up);
    };
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', up);
    target.addEventListener('pointercancel', up);
    event.preventDefault();
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.preventDefault();
      props.onClose();
    }
  }

  onMount(() => {
    document.addEventListener('keydown', onKeyDown);
    dialog?.focus();
    // The page behind must not scroll while a dialog is open: the wheel landed on the
    // thumbnail grid instead of the control under the pointer.
    const previous = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    onCleanup(() => {
      document.removeEventListener('keydown', onKeyDown);
      document.body.style.overflow = previous;
      // Closing the sheet stops any preview player it started.
      void ipc.stopPreview();
    });
  });

  function applyFill(preset: FillPreset): void {
    setValues('mode', preset.mode);
    if (preset.mode === 'gradient' && preset.gradient) setValues('gradient', preset.gradient);
    if (preset.mode === 'colour' && preset.color) setValues('color', preset.color);
  }

  /** Back to the values the tool started with, as the reference dialog offers. */
  function reset(): void {
    for (const field of props.fields) setValues(field.key, initialValue(field));
    setError('');
    setActionCount(0);
    setExtras({});
    setEditorReady(true);
    setResetToken((token) => token + 1);
  }

  /**
   * A hidden field is owned by a dedicated editor (the crop surface, the page grid,
   * a timeline): its value is still collected, but the generic control would be a
   * second, unused input for the same number.
   */
  const visibleFields = (): Field[] =>
    activeFields().filter((field) => isVisible(field) && !field.hidden);

  /**
   * Lossless encoders ignore `quality` (and the target-size search ignores it too),
   * so the control would do nothing: hide it, and keep it out of the parameters.
   */
  function activeFields(): Field[] {
    if (props.tool.id !== 'compress' || props.category !== 'images') return props.fields;
    const paths =
      fileList().length > 0
        ? fileList().map((file) => file.path)
        : props.previewPath
          ? [props.previewPath]
          : [];
    const lossless = isLosslessSelection(paths);
    const png = isPngSelection(paths);
    return props.fields.filter((field) => {
      if (lossless && field.key === 'quality') return false;
      // Palette reduction writes an indexed PNG; for any other output the engine
      // rejects `colours`, so the controls must not be shown (or collected).
      if (!png && (field.key === 'reduce' || field.key === 'colours')) return false;
      return true;
    });
  }

  /** A field hidden by its `show_if` must not travel: the backend rejects pairs. */
  function isVisible(field: Field): boolean {
    return visibleIn(field, values);
  }

  function collected(from: Record<string, FieldValue> = values): Record<string, unknown> {
    const params = collectParams(activeFields(), from);
    // The redaction style is a mode switch (a control), but the engine needs to know
    // which treatment the boxes get.
    if (props.tool.id === 'redact') params.style = String(from.style ?? 'solid');
    return params;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (submitting()) return;
    setSubmitting(true);
    setError('');
    try {
      const own = { ...collected(), ...extras() };
      const entries = fileList().map((file, index) => ({
        path: file.path,
        params: shared() || index === shown() ? own : (perFile.get(index) ?? { ...own }),
      }));
      await props.onSubmit(
        entries.length > 0
          ? entries
          : [{ path: props.previewPath ?? '', params: own }],
      );
      props.onClose();
    } catch (cause) {
      // The bridge rejects with a plain string; swallowing it produced a message that
      // told the user nothing about what to change.
      setError(
        cause instanceof Error
          ? cause.message
          : typeof cause === 'string'
            ? cause.replace(/^unsupported: /, '')
            : 'The backend rejected the request',
      );
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div
      class="modal-backdrop"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) props.onClose();
      }}
    >
      <div
        class="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sheet-title"
        tabindex="-1"
        ref={dialog}
      >
        <form
          class="dialog-sheet modal modal-form"
          ref={sheet}
          onSubmit={(event) => void submit(event)}
        >
        <span
          class="modal-grip"
          role="separator"
          aria-label="Resize this dialog from the corner"
          onPointerDown={(event) => beginResize(event, 'se')}
        >
          <svg viewBox="0 0 22 22" aria-hidden="true">
            <path
              d="M20 2 L2 20 M20 9 L9 20 M20 16 L16 20"
              fill="none"
              stroke="currentColor"
              stroke-width="1.4"
              stroke-linecap="round"
            />
          </svg>
        </span>
        <span
          class="modal-resize-e"
          role="separator"
          aria-label="Resize this dialog horizontally"
          onPointerDown={(event) => beginResize(event, 'e')}
        />
        <span
          class="modal-resize-s"
          role="separator"
          aria-label="Resize this dialog vertically"
          onPointerDown={(event) => beginResize(event, 's')}
        />
        <header
          class="sheet-head"
          onPointerDown={startMove}
          onDblClick={centre}
          title="Drag to move this dialog"
        >
          <h2 id="sheet-title" class="sheet-title">
            {props.tool.title ?? props.tool.label}
          </h2>
          <button type="button" class="sheet-close" aria-label="Close" onClick={() => props.onClose()}>
            ×
          </button>
        </header>
        <Show when={props.fileLabel}>
          <p class="modal-file">
            <span class="modal-file-name" title={props.fileLabel ?? ''}>
              {props.fileLabel}
            </span>
            <Show when={fileList().length > 1}>
              <div class="modal-files" role="group" aria-label="Which file">
                <button type="button" class="modal-file-step" aria-label="Previous file" onClick={() => go(-1)}>
                  ‹
                </button>
                <span class="modal-file-current" title={fileList()[shown()]?.label}>
                  {shown() + 1} of {fileList().length} · {fileList()[shown()]?.label}
                </span>
                <button type="button" class="modal-file-step" aria-label="Next file" onClick={() => go(1)}>
                  ›
                </button>
                <label class="modal-shared">
                  <input
                    type="checkbox"
                    checked={shared()}
                    onChange={(event) => setShared(event.currentTarget.checked)}
                  />
                  Same settings for every file
                </label>
              </div>
            </Show>
            <Show when={props.fileSize}>
                {(size) => <span class="modal-file-size">{formatBytes(size())}</span>}
              </Show>
            </p>
          </Show>
          <div class="modal-preview" classList={{ 'is-empty': !pinnedSurface() }}>
            <Show when={props.tool.id === 'compress' && props.category === 'images'}>
              <CompressPreview
                path={props.files?.[shown()]?.path ?? props.previewPath}
                params={collected()}
              />
            </Show>
            {/* Images crop: this surface. Video crop is the timeline surface below,
                which carries its own box over the first frame. */}
            <Show when={props.tool.id === 'crop' && props.category === 'images'}>
              <div class="field is-block">
                <span class="field-label">Area to keep</span>
                <CropEditor
                  previewPath={props.previewPath}
                  value={Array.isArray(values.boxes) ? ((values.boxes as Box[])[0] ?? null) : null}
                  aspect={String(values.aspect ?? 'free')}
                  onChange={(box) => setValues('boxes', [box])}
                />
              </div>
            </Show>
            <Show when={props.tool.id === 'redact' && props.category === 'images'}>
              <div class="field is-block">
                <span class="field-label">Areas to fill</span>
                {/* The surface shows the engine's own redaction, so the style, the
                    colour and the blur strength are visible before saving. */}
                <BoxesEditor
                  value={Array.isArray(values.boxes) ? (values.boxes as FieldValue) : []}
                  previewPath={props.previewPath}
                  effect={{
                    style: String(values.style ?? 'solid'),
                    color: String(values.color ?? '#000000'),
                    strength: Number(values.strength ?? 50),
                  }}
                  onChange={(next) => setValues('boxes', next)}
                />
              </div>
            </Show>
            {/* The annotation canvas is the dialog; its toolbar carries the ink and the
                shapes are the value the job burns in. */}
            <Show when={props.tool.id === 'annotate' && props.category === 'images'}>
              <div class="field is-block">
                <span class="field-label">Photo</span>
                <AnnotateEditor
                  previewPath={shownPath()}
                  value={values.shapes ?? []}
                  color={String(values.color ?? '#ff5a36')}
                  onInk={(color) => setValues('color', color)}
                  onChange={(next) => setValues('shapes', next)}
                />
              </div>
            </Show>
            <Show when={props.tool.id === 'add-background'}>
              <BackgroundPreview
                path={props.files?.[shown()]?.path ?? props.previewPath}
                mode={String(values.mode ?? 'colour')}
                color={String(values.color ?? '#101418')}
                gradient={String(values.gradient ?? '')}
                image={values.image ? String(values.image) : null}
                imageZoom={Number(values.bg_zoom ?? 1)}
                imageX={Number(values.bg_x ?? 0)}
                imageY={Number(values.bg_y ?? 0)}
                paddingPct={Number(values.padding_pct ?? 0)}
                radiusPct={Number(values.radius_pct ?? 0)}
                shadowPct={Number(values.shadow_pct ?? 0)}
                shadowAngle={Number(values.shadow_angle ?? 180)}
                bgBlur={Number(values.bg_blur ?? 0)}
                aspect={String(values.aspect ?? 'source')}
                onPlace={(x, y) => {
                  setValues('bg_x', x);
                  setValues('bg_y', y);
                }}
                onZoom={(zoom) => setValues('bg_zoom', zoom)}
              />
            </Show>
            <Show when={props.tool.id === 'edit-photo'}>
              <PhotoPreview
                path={props.files?.[shown()]?.path ?? props.previewPath}
                params={collected()}
              />
            </Show>
            <Show when={props.tool.id === 'add-background' && values.mode !== 'image'}>
              {/* One row: solids | gradients | another image. The separators are
                  part of the grammar, not decoration on a wrapped pile of swatches. */}
              <div class="fill-presets" role="group" aria-label="Ready-made backgrounds">
                <For each={SOLID_PRESETS}>
                  {(preset) => (
                    <button
                      type="button"
                      class="fill-preset"
                      style={{ background: preset.css }}
                      title={preset.label}
                      aria-label={preset.label}
                      onClick={() => applyFill(preset)}
                    />
                  )}
                </For>
                <span class="fill-separator" aria-hidden="true" />
                <For each={GRADIENT_PRESETS}>
                  {(preset) => (
                    <button
                      type="button"
                      class="fill-preset"
                      style={{ background: preset.css }}
                      title={preset.label}
                      aria-label={preset.label}
                      onClick={() => applyFill(preset)}
                    />
                  )}
                </For>
                <span class="fill-gap" aria-hidden="true" />
                <button
                  type="button"
                  class="fill-preset fill-preset-image"
                  title="Use another image"
                  aria-label="Use another image"
                  onClick={() => setValues('mode', 'image')}
                >
                  <svg viewBox="0 0 16 16" aria-hidden="true">
                    <rect x="1.5" y="2.5" width="13" height="11" rx="2" />
                    <circle cx="5.6" cy="6.4" r="1.3" />
                    <path d="M2.6 12.2l3.6-3.7 2.6 2.4 2.2-2.1 2.4 3.4" />
                  </svg>
                </button>
              </div>
            </Show>

            <Show when={props.tool.id === 'collage'}>
              <CollagePreview
                paths={fileList().map((file) => file.path)}
                params={collected()}
                onCells={(cells) =>
                  setExtras((previous) => ({ ...previous, cells }))
                }
                resetToken={resetToken()}
              />
            </Show>
          </div>
          <div class="modal-fields">
            <Show when={hasSurface()}>
              <Show when={props.category === 'video'}>
                <VideoEditor
                  tool={props.tool.id}
                  path={shownPath()}
                  values={values}
                  setValue={(key, value) => setValues(key, value)}
                  onExtras={setExtras}
                  onCount={setActionCount}
                  onReady={setEditorReady}
                  resetToken={resetToken()}
                />
              </Show>
              <Show when={props.category === 'audio'}>
                <AudioEditor
                  tool={props.tool.id}
                  path={shownPath()}
                  values={values}
                  setValue={(key, value) => setValues(key, value)}
                  collect={collected}
                  onExtras={setExtras}
                  onCount={setActionCount}
                  onReady={setEditorReady}
                  resetToken={resetToken()}
                />
              </Show>
            </Show>
            <For each={visibleFields()}>
              {(field, index) => (
                <>
                  <Show
                    when={
                      field.group &&
                      field.group !== visibleFields()[index() - 1]?.group
                    }
                  >
                    <h3 class="modal-group">{field.group}</h3>
                  </Show>
                  <FieldEditor
                    field={field}
                    value={values[field.key] ?? initialValue(field)}
                    previewPath={shownPath()}
                    files={fileList()}
                    ink={String(values.color ?? '#ff5a36')}
                    onInk={(color) => setValues('color', color)}
                    resetValue={initialValue(field)}
                    onChange={(value) => setValues(field.key, value)}
                  />
                </>
              )}
            </For>
          </div>
          <div class="sheet-foot">
            <Show when={error()}>
              <p class="field-error" role="alert">
                {error()}
              </p>
            </Show>
            <footer class="sheet-footer modal-footer">
              <button type="button" class="button button-quiet" onClick={reset}>
                Reset
              </button>
              <span class="modal-footer-gap" />
              {/* The primary button is named by the tool table, so the dialog says
                  "Save Organized PDF" rather than a generic "Run". */}
              <button
                type="submit"
                class="button button-primary"
                data-testid="primary"
                disabled={
                  submitting() ||
                  !editorReady() ||
                  ((props.tool.action ?? '').includes('{n}') && actionCount() === 0)
                }
              >
                {submitting() ? 'Starting…' : actionLabel(props.tool.action, actionCount())}
              </button>
            </footer>
          </div>
        </form>
      </div>
    </div>
  );
}
