import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
import { createStore } from 'solid-js/store';
import BackgroundPreview from './background-preview';
import CollagePreview from './collage-preview';
import CompressPreview from './compress-preview';
import CropEditor from './crop-editor';
import PhotoPreview from './photo-preview';
import FieldEditor, { BoxesEditor } from './field-editor';
import ToolInfo from './tool-info';
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
  /** Source pixels of the preview, for the crop dialog's W/H inputs. */
  const [natural, setNatural] = createSignal({ w: 0, h: 0 });
  let dialog: HTMLDivElement | undefined;
  let sheet: HTMLFormElement | undefined;

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

  /** The kept rectangle in source pixels, as the crop dialog shows it. */
  function cropPixels(): { w: number; h: number } {
    const size = natural();
    const box = Array.isArray(values.boxes) ? ((values.boxes as Box[])[0] ?? null) : null;
    const w = box ? box.w : 1;
    const h = box ? box.h : 1;
    return {
      w: size.w > 0 ? Math.round(w * size.w) : 0,
      h: size.h > 0 ? Math.round(h * size.h) : 0,
    };
  }

  /**
   * Editing a dimension moves the matching edge and keeps the origin, in fractions of
   * the source. The rectangle is clamped inside the picture and never collapses.
   */
  function setCropPixels(axis: 'w' | 'h', pixels: number): void {
    const size = natural();
    const total = axis === 'w' ? size.w : size.h;
    if (total <= 0 || !Number.isFinite(pixels) || pixels <= 0) return;
    const current = Array.isArray(values.boxes)
      ? ((values.boxes as Box[])[0] ?? { x: 0, y: 0, w: 1, h: 1 })
      : { x: 0, y: 0, w: 1, h: 1 };
    const fraction = Math.min(axis === 'w' ? 1 - current.x : 1 - current.y, pixels / total);
    if (fraction <= 0) return;
    setValues(
      'boxes',
      axis === 'w'
        ? [{ ...current, w: fraction }]
        : [{ ...current, h: fraction }],
    );
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
        aria-labelledby="rail-title"
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
          aria-label="Resize this window"
          onPointerDown={(event) => {
            const target = event.currentTarget;
            target.setPointerCapture(event.pointerId);
            const rect = sheet?.getBoundingClientRect();
            const start = { x: event.clientX, y: event.clientY, w: rect?.width ?? 600, h: rect?.height ?? 620 };
            const move = (e: PointerEvent): void => {
              const width = Math.min(window.innerWidth * 0.94, Math.max(440, start.w + (e.clientX - start.x)));
              const height = Math.min(window.innerHeight * 0.92, Math.max(320, start.h + (e.clientY - start.y)));
              sheet?.style.setProperty('--modal-w', `${Math.round(width)}px`);
              sheet?.style.setProperty('--modal-h', `${Math.round(height)}px`);
            };
            const up = (): void => {
              target.removeEventListener('pointermove', move);
              target.removeEventListener('pointerup', up);
              target.removeEventListener('pointercancel', up);
            };
            target.addEventListener('pointermove', move);
            target.addEventListener('pointerup', up);
            target.addEventListener('pointercancel', up);
          }}
        />
        <header class="sheet-head">
          <button type="button" class="sheet-close" aria-label="Close" onClick={() => props.onClose()}>
            ×
          </button>
          <h2 id="sheet-title" class="sheet-title">
            {props.tool.title ?? props.tool.label}
          </h2>
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
          <div class="modal-fields">
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
                  onSize={(size) => setNatural(size)}
                  onChange={(box) => setValues('boxes', [box])}
                />
              </div>
              <Show when={props.category === 'images'}>
                <div class="field crop-size">
                  <span class="field-label">Crop size</span>
                  <span class="crop-size-row">
                    <label for="crop-width">Width</label>
                    <input
                      id="crop-width"
                      type="number"
                      min="1"
                      step="1"
                      value={cropPixels().w}
                      onInput={(event) => setCropPixels('w', event.currentTarget.valueAsNumber)}
                    />
                    <label for="crop-height">Height</label>
                    <input
                      id="crop-height"
                      type="number"
                      min="1"
                      step="1"
                      value={cropPixels().h}
                      onInput={(event) => setCropPixels('h', event.currentTarget.valueAsNumber)}
                    />
                    <span class="field-unit">px</span>
                  </span>
                </div>
              </Show>
            </Show>
            <Show when={props.tool.id === 'redact' && props.category === 'images'}>
              <div class="field">
                <span class="field-label">Areas to fill</span>
                <BoxesEditor
                  value={Array.isArray(values.boxes) ? (values.boxes as FieldValue) : []}
                  previewPath={props.previewPath}
                  onChange={(next) => setValues('boxes', next)}
                />
              </div>
            </Show>
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
              <div class="fill-preset-rows">
                <div class="fill-presets" role="group" aria-label="Solid backgrounds">
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
                </div>
                <div class="fill-presets" role="group" aria-label="Gradient backgrounds">
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
              </div>
            </Show>

            <Show when={props.tool.id === 'collage'}>
              <CollagePreview
                paths={fileList().map((file) => file.path)}
                params={collected()}
              />
              <Show when={props.fields.find((field) => field.key === 'order')}>
                {(field) => (
                  <details class="collage-order" open>
                    <summary>Image order</summary>
                    <FieldEditor
                      field={field()}
                      value={values.order ?? initialValue(field())}
                      previewPath={shownPath()}
                      files={fileList()}
                      onChange={(value) => setValues('order', value)}
                    />
                  </details>
                )}
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
        <ToolInfo tool={props.tool} category={props.category} />
      </div>
    </div>
  );
}
