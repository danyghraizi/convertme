import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
import { createStore } from 'solid-js/store';
import BackgroundPreview from './background-preview';
import CropEditor from './crop-editor';
import PhotoPreview from './photo-preview';
import FieldEditor from './field-editor';
import { initialValue } from '../fields';
import { formatBytes } from '../format';
import type { Box, Field, FieldValue, Segment } from '../types';

interface ToolModalProps {
  tool: Segment;
  fields: Field[];
  /** Shown above the fields: the file the tool will act on, and its size. */
  fileLabel?: string | null;
  fileSize?: number | null;
  previewPath: string | null;
  onClose: () => void;
  onSubmit: (params: Record<string, unknown>) => Promise<void>;
}

export default function ToolModal(props: ToolModalProps) {
  const seed: Record<string, FieldValue> = {};
  for (const field of props.fields) seed[field.key] = initialValue(field);
  const [values, setValues] = createStore<Record<string, FieldValue>>(seed);
  const [submitting, setSubmitting] = createSignal(false);
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
    onCleanup(() => document.removeEventListener('keydown', onKeyDown));
  });

  const visibleFields = (): Field[] =>
    props.fields.filter((field) => isVisible(field) && field.key !== 'boxes');

  /** A field hidden by its `show_if` must not travel: the backend rejects pairs. */
  function isVisible(field: Field): boolean {
    const rule = field.show_if;
    if (!rule) return true;
    return String(values[rule.key] ?? '') === rule.equals;
  }

  function collected(): Record<string, unknown> {
    const params: Record<string, unknown> = {};
    for (const field of props.fields) {
      if (!isVisible(field) || field.control) continue;
      const value = values[field.key];
      // A select carries strings; a numeric choice (an image scale, a channel count)
      // is a number to the backend, and there is no way to tell the two apart from
      // the value alone, so a finite numeric string becomes a number.
      if (field.kind === 'select' && typeof value === 'string' && value.trim() !== '') {
        const numeric = Number(value);
        params[field.key] = Number.isFinite(numeric) ? numeric : value;
        continue;
      }
      params[field.key] = value;
    }
    return params;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (submitting()) return;
    setSubmitting(true);
    setError('');
    try {
      await props.onSubmit(collected());
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
        <form class="modal-form" onSubmit={(event) => void submit(event)}>
          <Show when={props.fileLabel}>
            <p class="modal-file">
              <span class="modal-file-name">{props.fileLabel}</span>
              <Show when={props.fileSize}>
                {(size) => <span class="modal-file-size">{formatBytes(size())}</span>}
              </Show>
            </p>
          </Show>
          <div class="modal-fields">
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
                path={props.previewPath}
                mode={String(values.mode ?? 'colour')}
                color={String(values.color ?? '#101418')}
                gradient={String(values.gradient ?? '')}
                image={values.image ? String(values.image) : null}
                padding={Number(values.padding ?? 0)}
                radius={Number(values.radius ?? 0)}
                aspect={String(values.aspect ?? 'source')}
              />
            </Show>
            <Show when={props.tool.id === 'edit-photo'}>
              <PhotoPreview
                path={props.previewPath}
                exposure={Number(values.exposure ?? 0)}
                brightness={Number(values.brightness ?? 0)}
                contrast={Number(values.contrast ?? 0)}
                saturation={Number(values.saturation ?? 0)}
                hue={Number(values.hue ?? 0)}
                temperature={Number(values.temperature ?? 0)}
              />
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
