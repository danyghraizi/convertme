import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
import { createStore } from 'solid-js/store';
import BackgroundPreview from './background-preview';
import CompressPreview from './compress-preview';
import CropEditor from './crop-editor';
import PhotoPreview from './photo-preview';
import FieldEditor from './field-editor';
import { collectParams, initialValue, visibleIn } from '../fields';
import { formatBytes, isLosslessSelection } from '../format';
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
  let dialog: HTMLDivElement | undefined;

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
  }

  const visibleFields = (): Field[] =>
    activeFields().filter((field) => isVisible(field) && field.key !== 'boxes');

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
    return isLosslessSelection(paths)
      ? props.fields.filter((field) => field.key !== 'quality')
      : props.fields;
  }

  /** A field hidden by its `show_if` must not travel: the backend rejects pairs. */
  function isVisible(field: Field): boolean {
    return visibleIn(field, values);
  }

  function collected(from: Record<string, FieldValue> = values): Record<string, unknown> {
    return collectParams(activeFields(), from);
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (submitting()) return;
    setSubmitting(true);
    setError('');
    try {
      const own = collected();
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
        class="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tool-modal-title"
        tabindex="-1"
        ref={dialog}
      >
        <header class="modal-head">
          <div>
            <h2 id="tool-modal-title">{props.tool.label}</h2>
            <p class="modal-blurb">{props.tool.blurb ?? ''}</p>
          </div>
        </header>
        <span
          class="modal-grip"
          role="separator"
          aria-label="Resize this window"
          onPointerDown={(event) => {
            const target = event.currentTarget;
            target.setPointerCapture(event.pointerId);
            const rect = dialog?.getBoundingClientRect();
            const start = { x: event.clientX, y: event.clientY, w: rect?.width ?? 640, h: rect?.height ?? 600 };
            const move = (e: PointerEvent): void => {
              const width = Math.min(window.innerWidth * 0.94, Math.max(420, start.w + (e.clientX - start.x)));
              const height = Math.min(window.innerHeight * 0.92, Math.max(320, start.h + (e.clientY - start.y)));
              dialog?.style.setProperty('--modal-w', `${Math.round(width)}px`);
              dialog?.style.setProperty('--modal-h', `${Math.round(height)}px`);
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
        <form class="modal-form" onSubmit={(event) => void submit(event)}>
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
            <Show when={props.tool.id === 'crop'}>
              <div class="field">
                <span class="field-label">Area to keep</span>
                <CropEditor
                  previewPath={props.previewPath}
                  value={Array.isArray(values.boxes) ? ((values.boxes as Box[])[0] ?? null) : null}
                  aspect={String(values.aspect ?? 'free')}
                  onChange={(box) => setValues('boxes', [box])}
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
                exposure={Number(values.exposure ?? 0)}
                brightness={Number(values.brightness ?? 0)}
                contrast={Number(values.contrast ?? 0)}
                saturation={Number(values.saturation ?? 0)}
                hue={Number(values.hue ?? 0)}
                temperature={Number(values.temperature ?? 0)}
              />
            </Show>
            <Show when={props.tool.id === 'add-background' && values.mode !== 'image'}>
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
                    previewPath={props.previewPath}
                    onChange={(value) => setValues(field.key, value)}
                  />
                </>
              )}
            </For>
          </div>
          <p class="field-error" role="alert">
            {error()}
          </p>
          <footer class="modal-footer">
            <button type="button" class="button button-quiet" onClick={reset}>
              Reset
            </button>
            <span class="modal-footer-gap" />
            <button type="button" class="button button-quiet" onClick={() => props.onClose()}>
              Cancel
            </button>
            <button type="submit" class="button button-primary" disabled={submitting()}>
              {submitting() ? 'Starting…' : 'Run'}
            </button>
          </footer>
        </form>
      </div>
    </div>
  );
}
