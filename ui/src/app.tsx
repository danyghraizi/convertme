import { For, Show, createEffect, createSignal, onCleanup, onMount } from 'solid-js';
import * as ipc from './ipc';
import HistoryList from './components/history-list';
import SettingsPanel from './components/settings-panel';
import ToolModal from './components/tool-modal';
import { CATEGORIES, FALLBACK_TOOLS, categoryLabel } from './catalog';
import { baseName, extensionOf } from './format';
import { mergePaths } from './paths';
import { keyLabel } from './keys';
import type {
  AnalyzeResult,
  BootInfo,
  DragSignal,
  Field,
  HistoryRow,
  JobEvent,
  ProgressState,
  RunResult,
  Segment,
  Settings,
  WheelContents,
} from './types';

interface ModalState {
  tool: Segment;
  fields: Field[];
  category: string;
  files: string[];
}

interface AlertState {
  title: string;
  message: string;
}

const HISTORY_LIMIT = 50;

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string' && error.trim()) return error;
  if (error && typeof error === 'object' && 'message' in error) {
    const value = error.message;
    if (typeof value === 'string' && value.trim()) return value;
  }
  return fallback;
}

function normalizeCategory(raw: string): string {
  return raw.toLowerCase();
}

export default function App() {
  const [bootInfo, setBootInfo] = createSignal<BootInfo | null>(null);
  const [settings, setSettings] = createSignal<Settings | null>(null);
  const [activeCategory, setActiveCategory] = createSignal(CATEGORIES[0].id);
  const [selection, setSelection] = createSignal<string[]>([]);
  const [analysis, setAnalysis] = createSignal<AnalyzeResult | null>(null);
  const [tools, setTools] = createSignal<Segment[] | null>(null);
  const [targets, setTargets] = createSignal<Segment[] | null>(null);
  const [historyRows, setHistoryRows] = createSignal<HistoryRow[]>([]);
  const [progress, setProgress] = createSignal<Record<string, ProgressState>>({});
  const [view, setView] = createSignal<'workspace' | 'settings'>('workspace');
  const [activityOpen, setActivityOpen] = createSignal(false);
  const [modal, setModal] = createSignal<ModalState | null>(null);
  const [dragActive, setDragActive] = createSignal(false);
  const [analyzing, setAnalyzing] = createSignal(false);
  const [busyTool, setBusyTool] = createSignal('');
  const [note, setNote] = createSignal('');
  const [alert, setAlert] = createSignal<AlertState | null>(null);
  const [backendNote, setBackendNote] = createSignal('');
  const unlisteners: (() => void)[] = [];
  let disposed = false;
  let historyFrame = 0;
  let wheelToken = 0;

  const toolCards = (): Segment[] => {
    const list = tools();
    if (list && list.length > 0) return list;
    return FALLBACK_TOOLS[activeCategory()] ?? [];
  };
  const targetCards = (): Segment[] => targets() ?? [];
  const queuedCount = (): number => historyRows().filter((row) => row.status === 'queued').length;
  const runningCount = (): number => historyRows().filter((row) => row.status === 'running').length;
  const hasSelection = (): boolean => selection().length > 0;
  const settingsHint = (): string => {
    const current = settings();
    return `Hold ${keyLabel(current?.convert_key ?? 'shift')} to convert, ${keyLabel(
      current?.tools_key ?? 'control',
    )} for tools, ${keyLabel(current?.actions_key ?? 'alt')} for actions.`;
  };

  function track(promise: Promise<() => void>): void {
    void promise.then((unlisten) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    });
  }

  function setJobProgress(id: string, state: ProgressState | null): void {
    setProgress((current) => {
      const next = { ...current };
      if (state) next[id] = state;
      else delete next[id];
      return next;
    });
  }

  function reportError(title: string, error: unknown, fallback: string): void {
    const message = errorMessage(error, fallback);
    setAlert({ title, message });
    setNote(message);
  }

  function scheduleHistory(): void {
    if (historyFrame !== 0) return;
    historyFrame = requestAnimationFrame(() => {
      historyFrame = 0;
      void refreshHistory();
    });
  }

  async function refreshHistory(): Promise<void> {
    const rows = await ipc.history(HISTORY_LIMIT).catch(() => []);
    setHistoryRows(rows);
  }

  function onJobEvent(event: JobEvent): void {
    if (event.kind === 'idle') {
      scheduleHistory();
      return;
    }
    const id = event.id;
    if (!id) return;
    switch (event.kind) {
      case 'queued':
        setJobProgress(id, { status: 'queued', fraction: 0 });
        scheduleHistory();
        break;
      case 'started':
        setJobProgress(id, {
          status: 'running',
          fraction: 0,
          stage: event.stage,
        });
        scheduleHistory();
        break;
      case 'progress':
        setJobProgress(id, {
          status: 'running',
          fraction: event.fraction ?? 0,
          stage: event.stage,
        });
        break;
      case 'finished':
      case 'failed':
      case 'cancelled':
        setJobProgress(id, null);
        if (event.kind === 'failed' && event.error) {
          reportError(
            'The job failed',
            `${event.error.code}: ${event.error.message}`,
            'The job failed.',
          );
        }
        scheduleHistory();
        break;
      default:
        break;
    }
  }

  function onDragSignal(signal: DragSignal): void {
    // The backend shows the wheel itself when a modifier goes down; the only
    // thing the main window needs to surface is a backend that cannot observe
    // drags at all (Wayland without the portal, for example).
    if (signal.kind === 'unavailable') setBackendNote(signal.reason);
  }

  async function refreshWheelData(): Promise<void> {
    const token = ++wheelToken;
    const category = activeCategory();
    const format = analysis()?.category === category ? (analysis()?.format ?? null) : null;
    const empty: WheelContents = { convert: [], tools: [], actions: [] };
    const [toolsResult, targetsResult] = await Promise.all([
      ipc.wheels(category, null).catch(() => empty),
      ipc.wheels(category, format).catch(() => empty),
    ]);
    if (token !== wheelToken) return;
    setTools(toolsResult.tools);
    setTargets(targetsResult.convert);
  }

  async function handlePaths(next: string[]): Promise<void> {
    if (next.length === 0) return;
    const merged = mergePaths(selection(), next);
    setSelection(merged);
    setModal(null);
    setAnalyzing(true);
    const result = await ipc.analyze(merged).catch(() => null);
    setAnalyzing(false);
    if (!result) {
      setAnalysis(null);
      setTargets([]);
      setNote('Could not analyse the dropped files');
      return;
    }
    setAnalysis(result);
    if (result.category) setActiveCategory(normalizeCategory(result.category));
    await refreshWheelData();
    const skipped = result.skipped.length;
    setNote(
      skipped > 0
        ? `${result.count} readable file${result.count === 1 ? '' : 's'}, ${skipped} skipped`
        : '',
    );
  }

  /** Re-analyse the remaining files after the selection changes. */
  async function replaceSelection(next: string[]): Promise<void> {
    setSelection(next);
    setModal(null);
    if (next.length === 0) {
      setAnalysis(null);
      setTargets([]);
      setNote('Selection cleared');
      return;
    }
    setAnalyzing(true);
    const result = await ipc.analyze(next).catch(() => null);
    setAnalyzing(false);
    if (!result) {
      setAnalysis(null);
      setNote('Could not read the remaining files');
      return;
    }
    setAnalysis(result);
    if (result.category) setActiveCategory(normalizeCategory(result.category));
    await refreshWheelData();
  }

  async function removeFile(path: string): Promise<void> {
    await replaceSelection(selection().filter((item) => item !== path));
  }

  async function clearFiles(): Promise<void> {
    await replaceSelection([]);
  }

  async function ensureSelection(): Promise<string[] | null> {
    if (hasSelection()) return selection();
    const picked = await ipc.pickFiles().catch(() => []);
    if (picked.length === 0) return null;
    await handlePaths(picked);
    return picked;
  }

  async function chooseFiles(): Promise<void> {
    const picked = await ipc.pickFiles().catch(() => []);
    if (picked.length > 0) await handlePaths(picked);
  }

  function announceRun(result: RunResult, label: string): void {
    const skipped = result.skipped.length;
    setNote(
      [
        `${result.jobs.length} job${result.jobs.length === 1 ? '' : 's'} started (${label})`,
        skipped > 0 ? `${skipped} skipped` : null,
      ]
        .filter((part): part is string => part !== null)
        .join(' \u00b7 '),
    );
  }

  async function convertWith(target: Segment): Promise<void> {
    const files = selection();
    if (files.length === 0 || target.disabled) return;
    try {
      const result = await ipc.run(files, activeCategory(), {
        kind: 'convert',
        target: target.id,
      });
      announceRun(result, target.label);
      scheduleHistory();
    } catch (error) {
      reportError('Conversion could not start', error, 'The conversion could not start.');
    }
  }

  async function openTool(tool: Segment): Promise<void> {
    if (tool.disabled || busyTool().length > 0) return;
    const files = await ensureSelection();
    if (!files) return;
    const category = activeCategory();
    setBusyTool(tool.id);
    try {
      const fields = await ipc.toolFields(category, tool.id).catch(() => []);
      if (fields.length === 0) {
        const result = await ipc.run(files, category, {
          kind: 'tool',
          tool: tool.id,
          params: {},
        });
        announceRun(result, tool.label);
        scheduleHistory();
        return;
      }
      setModal({ tool, fields, category, files });
    } catch (error) {
      reportError('Tool could not start', error, 'The tool could not start.');
    } finally {
      setBusyTool('');
    }
  }

  async function submitModal(
    entries: { path: string; params: Record<string, unknown> }[],
  ): Promise<void> {
    const state = modal();
    if (!state) return;
    let announced = 0;
    // Merge, Join and Collage read every input in one job: one document out of the
    // whole selection. Running them per file produced one artifact per input instead.
    if (state.tool.id === 'merge' || state.tool.id === 'join' || state.tool.id === 'collage') {
      const params = entries[0]?.params ?? {};
      const result = await ipc.run(
        entries.map((entry) => entry.path),
        state.category,
        { kind: 'tool', tool: state.tool.id, params },
      );
      announceRun(result, state.tool.label);
      scheduleHistory();
      return;
    }
    for (const entry of entries) {
      const params = { ...entry.params };
      if (state.tool.id === 'crop') {
        const boxes = Array.isArray(params.boxes) ? (params.boxes as Record<string, number>[]) : [];
        delete params.boxes;
        const box = boxes[0];
        if (box) {
          // The rectangle the user placed is the crop, whatever the preset says. Sending
          // the ratio as well made the engine ignore the placement and crop the centre,
          // which the preset box happened to satisfy — the reported bug.
          delete params.aspect;
          params.x = box.x;
          params.y = box.y;
          params.w = box.w;
          params.h = box.h;
        } else {
          // No rectangle (a job built without the editor) can only state the ratio. The
          // engine rejects both together, so exactly one of the two shapes travels.
          if (params.aspect === 'free') delete params.aspect;
        }
      }
      const result = await ipc.run([entry.path], state.category, {
        kind: 'tool',
        tool: state.tool.id,
        params,
      });
      announceRun(result, state.tool.label);
      announced += result.jobs.length;
    }
    if (announced === 0) announceRun({ jobs: [], skipped: [] }, state.tool.label);
    scheduleHistory();
  }

  async function saveSettings(next: Settings): Promise<void> {
    const previous = settings();
    if (previous && previous.start_at_login !== next.start_at_login) {
      // The login entry lives outside our settings file; create or remove it first,
      // so the toggle cannot claim something the system will not do.
      await ipc.setAutostart(next.start_at_login);
    }
    await ipc.saveSettings(next);
    setSettings(next);
    setNote('Settings saved');
  }

  function selectCategory(id: string): void {
    if (id === activeCategory()) return;
    setActiveCategory(id);
    setTargets(null);
    setTools(null);
    void refreshWheelData();
  }

  async function refreshAutostart(): Promise<void> {
    const current = settings();
    if (!current) return;
    const actual = await ipc.autostartStatus().catch(() => current.start_at_login);
    if (actual !== current.start_at_login) {
      setSettings({ ...current, start_at_login: actual });
    }
  }

  // Reconcile the login toggle with the system every time the page is opened.
  createEffect(() => {
    if (view() === 'settings') void refreshAutostart();
  });

  async function refreshBackendInfo(): Promise<void> {
    const info = await ipc.boot().catch(() => null);
    if (!info) return;
    setBootInfo(info);
    await refreshWheelData();
    if (info.pending_paths.length > 0) void handlePaths(info.pending_paths);
  }

  onMount(() => {
    document.documentElement.dataset.window = 'main';
    void ipc.boot().then(
      (info) => {
        setBootInfo(info);
        setSettings(info.settings);
        if (info.pending_paths.length > 0) {
          void handlePaths(info.pending_paths);
        } else {
          void ipc.takePendingPaths().then((late) => {
            if (late.length > 0) void handlePaths(late);
          });
        }
      },
      () => setNote('Backend did not answer boot'),
    );
    void refreshHistory();
    void refreshWheelData();

    track(ipc.onEvent('open-paths', (paths) => void handlePaths(paths)));
    track(ipc.onEvent('capabilities-changed', () => void refreshBackendInfo()));
    track(ipc.onEvent('job-event', (event) => onJobEvent(event)));
    track(ipc.onEvent('drag-signal', (signal) => onDragSignal(signal)));
    track(
      ipc.subscribeDragDrop((payload) => {
        if (payload.kind === 'enter' || payload.kind === 'over') setDragActive(true);
        else if (payload.kind === 'leave') setDragActive(false);
        else if (payload.kind === 'drop') {
          setDragActive(false);
          void handlePaths(payload.paths);
        }
      }),
    );

    // One event, once the first frame has really been painted: this is what
    // `cargo xtask perf start` measures.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        void ipc.emitToBackend('ui-ready');
      });
    });

    // The perf harness measures from process start to this frame.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (!disposed) void ipc.emitToBackend('ui-ready');
      });
    });

    onCleanup(() => {
      disposed = true;
      if (historyFrame !== 0) cancelAnimationFrame(historyFrame);
      for (const unlisten of unlisteners) unlisten();
      unlisteners.length = 0;
      delete document.documentElement.dataset.window;
    });
  });

  const activeJobs = (): number =>
    historyRows().filter((row) => row.status === 'running' || row.status === 'queued').length;

  const activitySummary = (): string => {
    const rows = historyRows();
    if (rows.length === 0) return 'nothing yet';
    const done = rows.filter((row) => row.status === 'done').length;
    const failed = rows.filter(
      (row) => row.status === 'failed' || row.status === 'interrupted',
    ).length;
    const active = rows.filter((row) => row.status === 'running' || row.status === 'queued').length;
    return [
      active > 0 ? `${active} active` : null,
      `${done} done`,
      failed > 0 ? `${failed} failed` : null,
    ]
      .filter((part): part is string => part !== null)
      .join(' · ');
  };

  return (
    <div class="app">
      <aside class="sidebar-nav" aria-label="Categories and settings">
        <div class="brand">
          <h1>{bootInfo()?.about.name ?? 'ConvertMe'}</h1>
          <span class="brand-version">
            {bootInfo() ? `v${bootInfo()?.about.version}` : 'starting'}
          </span>
        </div>

        <nav class="category-bar" aria-label="Categories">
          <For each={CATEGORIES}>
            {(category) => (
              <button
                type="button"
                class="category-button"
                classList={{ 'is-active': activeCategory() === category.id }}
                aria-pressed={activeCategory() === category.id}
                onClick={() => selectCategory(category.id)}
              >
                <span>{category.label}</span>
              </button>
            )}
          </For>
        </nav>

        <div class="sidebar-foot">
          <button
            type="button"
            class="button"
            classList={{ 'is-active': view() === 'settings' }}
            aria-pressed={view() === 'settings'}
            onClick={() => setView(view() === 'settings' ? 'workspace' : 'settings')}
          >
            <svg
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.4"
              aria-hidden="true"
            >
              <circle cx="8" cy="8" r="2.4" />
              <path
                d="M8 1.6v1.7M8 12.7v1.7M1.6 8h1.7M12.7 8h1.7M3.5 3.5l1.2 1.2M11.3 11.3l1.2 1.2M12.5 3.5l-1.2 1.2M4.7 11.3l-1.2 1.2"
                stroke-linecap="round"
              />
            </svg>
            Settings
          </button>
        </div>
      </aside>

      <div class="app-main">
        <Show when={view() === 'settings'}>
          <section class="settings-page" aria-label="Settings">
            <Show when={settings()}>
              {(current) => (
                <SettingsPanel
                  settings={current()}
                  backend={bootInfo()?.backend ?? null}
                  about={bootInfo()?.about ?? null}
                  missing={bootInfo()?.capabilities.missing ?? []}
                  onSave={saveSettings}
                  onClose={() => setView('workspace')}
                />
              )}
            </Show>
          </section>
        </Show>

        <main
          class="workspace"
          aria-label="Workspace"
          style={{ display: view() === 'settings' ? 'none' : undefined }}
        >
          <div
            class="dropzone"
            classList={{
              'is-dragging': dragActive(),
              'is-filled': hasSelection(),
            }}
          >
            <div class="dropzone-text">
              <p class="dropzone-title">
                {analyzing()
                  ? 'Reading files'
                  : hasSelection()
                    ? `${selection().length} file${selection().length === 1 ? '' : 's'} ready`
                    : 'Drop files here'}
              </p>
              <p class="dropzone-copy">
                {hasSelection()
                  ? settingsHint()
                  : 'Drag a file from the file manager, or choose one below. Everything stays on this machine.'}
              </p>
            </div>
            <button type="button" class="button" onClick={() => void chooseFiles()}>
              Choose files…
            </button>
          </div>

          <Show when={alert()}>
            {(item) => (
              <div class="alert-banner" role="alert" aria-live="assertive">
                <div class="alert-copy">
                  <span><strong>{item().title}: </strong>{item().message}</span>
                </div>
                <button
                  type="button"
                  class="alert-dismiss"
                  aria-label="Dismiss error"
                  onClick={() => setAlert(null)}
                >
                  Dismiss
                </button>
              </div>
            )}
          </Show>

          <Show when={analysis()}>
            {(result) => (
              <section class="panel panel-card selection-panel" aria-labelledby="selection-heading">
                <header class="panel-head">
                  <h2 id="selection-heading">Selected</h2>
                  <span class="panel-meta">
                    {categoryLabel(result().category)}
                    {result().format ? ` · ${result().format}` : ''}
                    {result().count > 0
                      ? ` · ${result().count} file${result().count === 1 ? '' : 's'}`
                      : ''}
                  </span>
                  <button
                    type="button"
                    class="button button-quiet panel-action"
                    onClick={() => void clearFiles()}
                  >
                    Clear
                  </button>
                </header>
                <ul class="thumb-strip">
                  <For each={result().thumbs}>
                    {(thumb) => (
                      <li class="thumb">
                        <Show
                          when={thumb.thumb}
                          fallback={
                            <span class="thumb-blank">
                              {extensionOf(thumb.path).toUpperCase() || 'FILE'}
                            </span>
                          }
                        >
                          <img src={ipc.assetUrl(thumb.thumb)} alt="" loading="lazy" />
                        </Show>
                        <button
                          type="button"
                          class="thumb-remove"
                          aria-label={`Remove ${baseName(thumb.path)} from the selection`}
                          title="Remove from the selection"
                          onClick={() => void removeFile(thumb.path)}
                        >
                          ×
                        </button>
                        <span class="thumb-name" title={thumb.path}>
                          {baseName(thumb.path)}
                        </span>
                      </li>
                    )}
                  </For>
                </ul>
                <Show when={result().skipped.length > 0}>
                  <details class="skipped">
                    <summary>{result().skipped.length} skipped</summary>
                    <ul>
                      <For each={result().skipped}>
                        {(item) => (
                          <li>
                            <span title={item.path}>{baseName(item.path)}</span>
                            <span class="skipped-message">{item.message}</span>
                          </li>
                        )}
                      </For>
                    </ul>
                  </details>
                </Show>
              </section>
            )}
          </Show>

          <Show when={hasSelection()}>
            <section class="panel panel-card targets-panel" aria-labelledby="targets-heading">
              <header class="panel-head">
                <h2 id="targets-heading">Convert to</h2>
                <span class="panel-meta">{categoryLabel(activeCategory())}</span>
              </header>
              <Show
                when={targetCards().length > 0}
                fallback={
                  <p class="empty-note">
                    {targets() === null ? 'Loading targets…' : 'No conversion for this format.'}
                  </p>
                }
              >
                <ul class="target-grid">
                  <For each={targetCards()}>
                    {(target) => (
                      <li>
                        <button
                          type="button"
                          class="target-card"
                          disabled={target.disabled === true}
                          title={target.blurb ?? target.label}
                          onClick={() => void convertWith(target)}
                        >
                          <span class="target-label">{target.label}</span>
                          <Show when={target.hint}>
                            <span class="target-blurb">{target.hint}</span>
                          </Show>
                        </button>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </section>
          </Show>

          <section class="panel panel-card tools-panel" aria-labelledby="tools-heading">
            <header class="panel-head">
              <h2 id="tools-heading">Tools</h2>
              <span class="panel-meta">
                {categoryLabel(activeCategory())} · {toolCards().length} available
              </span>
            </header>
            <Show when={toolCards().length === 0}>
              <p class="empty-note">
                {hasSelection()
                  ? 'No tools for this file type yet. Use the format buttons above.'
                  : 'Choose a file to see the tools for its format.'}
              </p>
            </Show>
            <ul class="tool-grid">
              <For each={toolCards()}>
                {(tool, index) => (
                  <li>
                    <button
                      type="button"
                      class="tool-card"
                      classList={{ 'is-busy': busyTool() === tool.id }}
                      disabled={tool.disabled === true}
                      onClick={() => void openTool(tool)}
                    >
                      <span class="tool-index">{String(index() + 1).padStart(2, '0')}</span>
                      <span class="tool-text">
                        <span class="tool-label">{tool.label}</span>
                        <span class="tool-blurb">
                          {busyTool() === tool.id ? 'Reading fields…' : (tool.blurb ?? '')}
                        </span>
                        <Show when={tool.hint}>
                          <span class="tool-hint">{tool.hint}</span>
                        </Show>
                      </span>
                      <span class="tool-chevron" aria-hidden="true">
                        <svg
                          viewBox="0 0 16 16"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="1.6"
                        >
                          <path d="M6 4l4 4-4 4" stroke-linecap="round" stroke-linejoin="round" />
                        </svg>
                      </span>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </section>
        </main>

        <section
          class="activity"
          classList={{ 'is-collapsed': !activityOpen() }}
          aria-label="Activity"
        >
          <button
            type="button"
            class="activity-bar"
            aria-expanded={activityOpen()}
            onClick={() => setActivityOpen((open) => !open)}
          >
            <span class="activity-title">Activity</span>
            <span class="activity-summary">{activitySummary()}</span>
            <Show when={historyRows().length > 0}>
              <span class="activity-spark" aria-hidden="true">
                {/* History is newest-first: the dot on the left is the latest job. */}
                <For each={historyRows().slice(0, 12)}>
                  {(row) => <span class="spark-dot" data-status={row.status} />}
                </For>
              </span>
            </Show>
            <span class="activity-spacer" />
            <Show when={activityOpen() && activeJobs() > 0}>
              <span
                class="activity-cancel"
                role="button"
                tabindex="0"
                onClick={(event) => {
                  event.stopPropagation();
                  void ipc
                    .cancelAll()
                    .then(scheduleHistory, () => setNote('Could not cancel jobs'));
                }}
              >
                Cancel all
              </span>
            </Show>
            <span class="activity-toggle" aria-hidden="true">
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
                <path d="M4 6l4 4 4-4" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
            </span>
          </button>
          <div class="activity-body">
            <HistoryList
              rows={historyRows()}
              progress={progress()}
              limit={HISTORY_LIMIT}
              onCancel={(id) => {
                void ipc.cancelJob(id).then(scheduleHistory, (error: unknown) => {
                  reportError('Could not cancel the job', error, 'The job could not be cancelled.');
                });
              }}
              onRetry={(id) => {
                void ipc.retry(id).then(scheduleHistory, (error: unknown) => {
                  reportError('Could not retry the job', error, 'The job could not be retried.');
                });
              }}
              onCancelAll={() => {
                void ipc.cancelAll().then(scheduleHistory, (error: unknown) => {
                  reportError('Could not cancel jobs', error, 'The jobs could not be cancelled.');
                });
              }}
            />
          </div>
        </section>

        <footer class="status-strip" role="status" aria-live="polite">
          <span class="status-counts">
            {queuedCount()} queued · {runningCount()} running
          </span>
          <span class="status-divider" aria-hidden="true" />
          <span class="status-backend">
            {bootInfo()?.backend.kind ?? 'connecting'}
            {bootInfo()?.about.options ? ` · ${bootInfo()?.about.options} options` : ''}
          </span>
          <Show when={note()}>
            <span class="status-note">{note()}</span>
          </Show>
          <Show when={backendNote()}>
            <span class="status-warning">{backendNote()}</span>
          </Show>
          <button
            type="button"
            class="status-link"
            title="Open the wheel for this selection (same as pressing the key during a drag)"
            onClick={() => {
              if (ipc.isMock()) {
                window.open('/?window=wheel', 'convertme-wheel', 'width=680,height=680');
                return;
              }
              void ipc.previewWheel(selection()).catch((error) => {
                reportError('The wheel could not open', error, 'The preview wheel could not open.');
              });
            }}
          >
            Preview wheel
          </button>
        </footer>
      </div>

      <Show when={modal()}>
        {(state) => (
          <ToolModal
            tool={state().tool}
            fields={state().fields}
            category={state().category}
            fileLabel={
              state().files.length === 1
                ? baseName(state().files[0])
                : `${state().files.length} files`
            }
            fileSize={
              state().files.length === 1
                ? (analysis()?.thumbs.find((thumb) => thumb.path === state().files[0])?.size ??
                  null)
                : null
            }
            files={state().files.map((path) => ({ path, label: baseName(path) }))}
            previewPath={state().files[0] ?? null}
            onClose={() => setModal(null)}
            onSubmit={submitModal}
          />
        )}
      </Show>
    </div>
  );
}
