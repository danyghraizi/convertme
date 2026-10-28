import { For, createSignal, onCleanup, onMount } from 'solid-js';
import { createStore } from 'solid-js/store';
import FieldEditor from './field-editor';
import { initialValue } from '../fields';
import type { Field, FieldValue, Segment } from '../types';

interface ToolModalProps {
  tool: Segment;
  fields: Field[];
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

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (submitting()) return;
    setSubmitting(true);
    setError('');
    try {
      await props.onSubmit({ ...values });
      props.onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The backend rejected the request');
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
          <div class="modal-fields">
            <For each={props.fields}>
              {(field) => (
                <FieldEditor
                  field={field}
                  value={values[field.key] ?? initialValue(field)}
                  previewPath={props.previewPath}
                  onChange={(value) => setValues(field.key, value)}
                />
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
