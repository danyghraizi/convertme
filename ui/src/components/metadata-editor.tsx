import { For, Show, createSignal, onMount } from 'solid-js';
import * as ipc from '../ipc';
import { asMeta } from '../fields';
import type { FieldValue, MetaFields, MetaGroup } from '../types';
import './metadata-editor.css';

interface MetadataEditorProps {
  /** The file whose tags are being edited. */
  path: string | null;
  value: FieldValue;
  onChange: (value: MetaFields) => void;
}

/**
 * The metadata browser: file facts, then every tag the writer can change.
 *
 * The table comes from the backend (`file_metadata`), which is also what decides
 * whether a row can be removed: a writer without a per-tag delete reports the row as
 * read-only, so there is no minus button that would fail. Edits are the `set` map,
 * a minus is a `remove` entry, and "Remove All" is `strip` — one value, three keys,
 * flattened by `collectParams`.
 */
export default function MetadataEditor(props: MetadataEditorProps) {
  const [groups, setGroups] = createSignal<MetaGroup[]>([]);
  const [query, setQuery] = createSignal('');
  const [note, setNote] = createSignal('');

  onMount(() => {
    const path = props.path;
    if (!path) {
      setNote('Choose a file to see its metadata.');
      return;
    }
    void ipc
      .fileMetadata(path)
      .then((loaded) => setGroups(loaded))
      .catch((error) => setNote(error instanceof Error ? error.message : 'Metadata unavailable'));
  });

  const meta = (): MetaFields => asMeta(props.value);

  function edited(key: string, text: string): void {
    const current = meta();
    props.onChange({
      ...current,
      set: { ...current.set, [key]: text },
      remove: current.remove.filter((entry) => entry !== key),
    });
  }

  function drop(key: string): void {
    const current = meta();
    const set = { ...current.set };
    delete set[key];
    props.onChange({
      ...current,
      set,
      remove: current.remove.includes(key) ? current.remove : [...current.remove, key],
    });
  }

  function keep(key: string): void {
    const current = meta();
    props.onChange({ ...current, remove: current.remove.filter((entry) => entry !== key) });
  }

  function rowsFor(group: MetaGroup) {
    const needle = query().trim().toLowerCase();
    if (!needle) return group.fields;
    return group.fields.filter(
      (field) =>
        field.label.toLowerCase().includes(needle) ||
        field.value.toLowerCase().includes(needle) ||
        (meta().set[field.key] ?? '').toLowerCase().includes(needle),
    );
  }

  return (
    <div class="metadata-editor">
      <div class="metadata-head">
        <button
          type="button"
          class="metadata-remove-all"
          aria-pressed={meta().strip}
          aria-label="Remove All"
          onClick={() => props.onChange({ ...meta(), strip: !meta().strip })}
        >
          {meta().strip ? 'Keep every tag' : 'Remove all'}
        </button>
        <input
          class="metadata-search"
          type="search"
          placeholder="Search metadata"
          aria-label="Search metadata"
          value={query()}
          onInput={(event) => setQuery(event.currentTarget.value)}
        />
      </div>
      <Show when={note()}>
        <p class="field-help" role="status">
          {note()}
        </p>
      </Show>
      <Show when={meta().strip}>
        <p class="metadata-note" role="status">
          Every tag will be removed when this is saved.
        </p>
      </Show>
      <For each={groups()}>
        {(group) => (
          <Show when={rowsFor(group).length > 0}>
            <details class="metadata-group" open>
              <summary class="metadata-group-name">{group.name}</summary>
              <ul class="metadata-rows">
                <For each={rowsFor(group)}>
                  {(field) => {
                    const removed = (): boolean => meta().remove.includes(field.key);
                    const value = (): string => meta().set[field.key] ?? field.value;
                    return (
                      <li
                        class="metadata-row"
                        classList={{ 'is-removed': removed(), 'is-readonly': !field.editable }}
                      >
                        <label class="metadata-label" for={`meta-${group.name}-${field.key}`}>
                          {field.label}
                        </label>
                        <Show
                          when={field.editable}
                          fallback={<span class="metadata-value">{field.value || '—'}</span>}
                        >
                          <input
                            id={`meta-${group.name}-${field.key}`}
                            class="metadata-input"
                            type="text"
                            aria-label={field.label}
                            value={value()}
                            disabled={removed()}
                            onInput={(event) => edited(field.key, event.currentTarget.value)}
                          />
                          <Show
                            when={removed()}
                            fallback={
                              <button
                                type="button"
                                class="metadata-remove"
                                aria-label={`Remove ${field.key}`}
                                onClick={() => drop(field.key)}
                              >
                                −
                              </button>
                            }
                          >
                            <button
                              type="button"
                              class="metadata-remove is-undo"
                              aria-label={`Keep ${field.key}`}
                              onClick={() => keep(field.key)}
                            >
                              ↺
                            </button>
                          </Show>
                        </Show>
                      </li>
                    );
                  }}
                </For>
              </ul>
            </details>
          </Show>
        )}
      </For>
      <Show when={groups().length > 0 && query().trim() !== '' && groups().every((g) => rowsFor(g).length === 0)}>
        <p class="field-help">No field matches “{query()}”.</p>
      </Show>
    </div>
  );
}
