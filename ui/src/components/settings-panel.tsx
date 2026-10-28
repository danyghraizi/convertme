import { For, Show, createMemo, createSignal } from 'solid-js';
import { createStore } from 'solid-js/store';
import type {
  AboutInfo,
  BackendInfo,
  MissingTool,
  OverwritePolicy,
  Settings,
} from '../types';
import { WHEEL_KEYS, keyLabel } from '../keys';
import { formatBytes } from '../format';

interface SettingsPanelProps {
  settings: Settings;
  backend: BackendInfo | null;
  about: AboutInfo | null;
  missing: MissingTool[];
  onSave: (settings: Settings) => Promise<void>;
  onClose: () => void;
}

const OVERWRITE_OPTIONS: { value: OverwritePolicy; label: string }[] = [
  { value: 'never', label: 'Never overwrite (add a suffix)' },
  { value: 'ask', label: 'Ask before replacing' },
  { value: 'replace', label: 'Replace existing outputs' },
];

const DRAG_BACKENDS = [
  { value: 'auto', label: 'Auto (detect the session)' },
  { value: 'windows', label: 'Windows hooks' },
  { value: 'x11', label: 'X11 drop target' },
  { value: 'wayland', label: 'Wayland drop overlay' },
];

export default function SettingsPanel(props: SettingsPanelProps) {
  const [draft, setDraft] = createStore<Settings>({
    ...props.settings,
    ocr_languages: [...props.settings.ocr_languages],
  });
  const [languagesText, setLanguagesText] = createSignal(draft.ocr_languages.join(', '));
  const [cacheMiB, setCacheMiB] = createSignal(
    Math.max(1, Math.round(draft.preview_cache_bytes / (1024 * 1024))),
  );
  const [saving, setSaving] = createSignal(false);
  const [message, setMessage] = createSignal('');

  const duplicateKeys = createMemo(
    () => new Set([draft.convert_key, draft.tools_key, draft.actions_key]).size !== 3,
  );
  const invalidLimit = createMemo(() => !Number.isFinite(draft.history_limit) || draft.history_limit < 1);

  function setWheelKey(field: 'convert_key' | 'tools_key' | 'actions_key', value: string): void {
    const key = WHEEL_KEYS.find((candidate) => candidate === value);
    if (key) setDraft(field, key);
  }

  async function save(): Promise<void> {
    if (duplicateKeys() || invalidLimit() || saving()) return;
    setSaving(true);
    setMessage('');
    try {
      await props.onSave({
        ...draft,
        history_limit: Math.round(draft.history_limit),
        preview_cache_bytes: Math.max(0, Math.round(cacheMiB())) * 1024 * 1024,
        extra_tools_dir:
          draft.extra_tools_dir && draft.extra_tools_dir.trim().length > 0
            ? draft.extra_tools_dir.trim()
            : null,
        ocr_languages: languagesText()
          .split(',')
          .map((language) => language.trim())
          .filter((language) => language.length > 0),
      });
      setMessage('Settings saved');
    } catch (error) {
      setMessage(error instanceof Error ? error.message : 'Could not save settings');
    } finally {
      setSaving(false);
    }
  }

  return (
    <section class="panel settings-panel" aria-labelledby="settings-heading">
      <header class="panel-head">
        <h2 id="settings-heading">Settings</h2>
        <button
          type="button"
          class="button button-quiet"
          onClick={() => props.onClose()}
          aria-label="Close settings"
        >
          Close
        </button>
      </header>

      <div class="settings-body">
        <fieldset class="settings-group">
          <legend>Wheel keys</legend>
          <p class="settings-help">
            Hold one of these keys while dragging to raise its wheel. All three must differ.
          </p>
          <div class="form-grid">
            <label class="field" for="settings-convert-key">
              <span class="field-label">Convert wheel</span>
              <select
                id="settings-convert-key"
                value={draft.convert_key}
                aria-invalid={duplicateKeys()}
                onChange={(event) => setWheelKey('convert_key', event.currentTarget.value)}
              >
                <For each={WHEEL_KEYS}>
                  {(key) => <option value={key}>{keyLabel(key)}</option>}
                </For>
              </select>
            </label>
            <label class="field" for="settings-tools-key">
              <span class="field-label">Tools wheel</span>
              <select
                id="settings-tools-key"
                value={draft.tools_key}
                aria-invalid={duplicateKeys()}
                onChange={(event) => setWheelKey('tools_key', event.currentTarget.value)}
              >
                <For each={WHEEL_KEYS}>
                  {(key) => <option value={key}>{keyLabel(key)}</option>}
                </For>
              </select>
            </label>
            <label class="field" for="settings-actions-key">
              <span class="field-label">Actions wheel</span>
              <select
                id="settings-actions-key"
                value={draft.actions_key}
                aria-invalid={duplicateKeys()}
                onChange={(event) => setWheelKey('actions_key', event.currentTarget.value)}
              >
                <For each={WHEEL_KEYS}>
                  {(key) => <option value={key}>{keyLabel(key)}</option>}
                </For>
              </select>
            </label>
          </div>
          <Show when={duplicateKeys()}>
            <p class="field-error" role="alert">
              Wheel keys must be different from each other.
            </p>
          </Show>
        </fieldset>

        <fieldset class="settings-group">
          <legend>Output</legend>
          <div class="form-grid">
            <label class="field" for="settings-overwrite">
              <span class="field-label">When the output exists</span>
              <select
                id="settings-overwrite"
                value={draft.overwrite}
                onChange={(event) => {
                  const value = event.currentTarget.value;
                  if (value === 'ask' || value === 'replace' || value === 'never') {
                    setDraft('overwrite', value);
                  }
                }}
              >
                <For each={OVERWRITE_OPTIONS}>
                  {(option) => <option value={option.value}>{option.label}</option>}
                </For>
              </select>
            </label>
            <label class="field" for="settings-redact">
              <span class="field-label">Redaction fill</span>
              <span class="color-row">
                <input
                  id="settings-redact"
                  type="color"
                  value={draft.redact_fill}
                  onInput={(event) => setDraft('redact_fill', event.currentTarget.value)}
                />
                <output>{draft.redact_fill}</output>
              </span>
            </label>
          </div>
        </fieldset>

        <fieldset class="settings-group">
          <legend>History and cache</legend>
          <div class="form-grid">
            <label class="field" for="settings-history-limit">
              <span class="field-label">History rows</span>
              <input
                id="settings-history-limit"
                type="number"
                min={1}
                max={10000}
                step={1}
                value={draft.history_limit}
                aria-invalid={invalidLimit()}
                onInput={(event) =>
                  setDraft('history_limit', Math.round(event.currentTarget.valueAsNumber))
                }
              />
            </label>
            <label class="field" for="settings-cache">
              <span class="field-label">Preview cache</span>
              <span class="number-row">
                <input
                  id="settings-cache"
                  type="number"
                  min={1}
                  max={65536}
                  step={16}
                  value={cacheMiB()}
                  onInput={(event) =>
                    setCacheMiB(Math.max(1, Math.round(event.currentTarget.valueAsNumber || 1)))
                  }
                />
                <span class="field-unit">MiB</span>
              </span>
              <span class="field-help">
                Cached thumbnails, filmstrips and waveforms; capped at{' '}
                {formatBytes(Math.max(0, cacheMiB()) * 1024 * 1024)}.
              </span>
            </label>
            <label class="field" for="settings-ocr">
              <span class="field-label">OCR languages</span>
              <input
                id="settings-ocr"
                type="text"
                value={languagesText()}
                placeholder="eng, deu, fra"
                onInput={(event) => setLanguagesText(event.currentTarget.value)}
              />
              <span class="field-help">Comma-separated Tesseract language codes.</span>
            </label>
          </div>
        </fieldset>

        <fieldset class="settings-group">
          <legend>Interaction</legend>
          <div class="form-grid">
            <label class="field" for="settings-drag">
              <span class="field-label">Drag backend</span>
              <select
                id="settings-drag"
                value={draft.drag_backend}
                onChange={(event) => setDraft('drag_backend', event.currentTarget.value)}
              >
                <For each={DRAG_BACKENDS}>
                  {(backend) => <option value={backend.value}>{backend.label}</option>}
                </For>
              </select>
            </label>
            <label class="field field-switch" for="settings-login">
              <span class="field-label">Start with the system</span>
              <span class="field-row">
                <input
                  id="settings-login"
                  type="checkbox"
                  checked={draft.start_at_login}
                  onChange={(event) => setDraft('start_at_login', event.currentTarget.checked)}
                />
                <span class="field-help">
                  Runs `convertme --background` at login so the wheel works without
                  opening the window.
                </span>
              </span>
            </label>
            <label class="field" for="settings-tools-dir">
              <span class="field-label">Extra tools directory</span>
              <input
                id="settings-tools-dir"
                type="text"
                value={draft.extra_tools_dir ?? ''}
                placeholder="Leave empty to use the bundled tools"
                onInput={(event) =>
                  setDraft(
                    'extra_tools_dir',
                    event.currentTarget.value.length > 0 ? event.currentTarget.value : null,
                  )
                }
              />
              <span class="field-help">Searched before the bundled tools.</span>
            </label>
            <label class="field field-checkbox" for="settings-update">
              <input
                id="settings-update"
                type="checkbox"
                checked={draft.update_check}
                onChange={(event) => setDraft('update_check', event.currentTarget.checked)}
              />
              <span class="field-label">Check for updates on launch</span>
            </label>
          </div>
        </fieldset>

        <fieldset class="settings-group">
          <legend>Backend</legend>
          <p class="settings-backend">
            <strong>{props.backend?.kind ?? 'unknown'}</strong>
            <span>{props.backend?.description ?? 'No backend information'}</span>
          </p>
          <Show when={props.about && props.about.options > 0}>
            <p class="settings-help">
              {props.about?.options} conversion options, {props.about?.tools} tools, version{' '}
              {props.about?.version}.
            </p>
          </Show>
          <Show
            when={props.missing.length > 0}
            fallback={<p class="settings-help">All optional tools were found.</p>}
          >
            <h3 class="settings-subhead">Missing tools</h3>
            <ul class="missing-list">
              <For each={props.missing}>
                {(tool) => (
                  <li>
                    <strong>{tool.label}</strong>
                    <span>{tool.hint}</span>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </fieldset>
      </div>

      <footer class="settings-footer">
        <p class="settings-message" role="status">
          {message()}
        </p>
        <button
          type="button"
          class="button button-primary"
          onClick={() => void save()}
          disabled={duplicateKeys() || invalidLimit() || saving()}
        >
          {saving() ? 'Saving' : 'Save settings'}
        </button>
      </footer>
    </section>
  );
}
