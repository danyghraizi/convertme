import { For, Show } from 'solid-js';
import type { HistoryRow, ProgressState } from '../types';
import { baseName, epochMillis, formatTime } from '../format';
import { isActiveStatus, isRetryableStatus } from '../job-status';

interface HistoryListProps {
  rows: HistoryRow[];
  progress: Record<string, ProgressState>;
  limit: number;
  onCancel: (id: string) => void;
  onRetry: (id: string) => void;
  onCancelAll: () => void;
}

export default function HistoryList(props: HistoryListProps) {
  return (
    <section class="history-panel" aria-label="Job history">
      <Show
        when={props.rows.length > 0}
        fallback={<p class="empty-note">Nothing converted yet. Drop a file to start.</p>}
      >
        <ul class="history-list">
          <For each={props.rows}>
            {(row) => {
              const state = (): ProgressState | undefined => props.progress[row.id];
              return (
                <li class="history-row" data-status={row.status}>
                  <span class="status-chip" data-status={row.status}>
                    {row.status}
                  </span>
                  <span class="history-name" title={row.input}>
                    {baseName(row.input)}
                  </span>
                  <time
                    class="history-time"
                    datetime={
                      row.started ? new Date(epochMillis(row.started)).toISOString() : undefined
                    }
                  >
                    {formatTime(row.started)}
                  </time>
                  <span class="history-intent">
                    {row.intent}
                    <Show when={row.status !== 'done'}>
                      <span class="history-status-word"> · {row.status}</span>
                    </Show>
                  </span>
                  <div class="history-actions">
                    <Show when={isActiveStatus(row.status)}>
                      <button
                        type="button"
                        class="button button-quiet"
                        onClick={() => props.onCancel(row.id)}
                      >
                        Cancel
                      </button>
                    </Show>
                    <Show when={isRetryableStatus(row.status)}>
                      <button
                        type="button"
                        class="button button-quiet"
                        onClick={() => props.onRetry(row.id)}
                      >
                        Retry
                      </button>
                    </Show>
                  </div>
                  <Show when={row.error_code}>
                    <p class="history-error" title={row.error_tail}>
                      {row.error_code}
                      {row.error_tail ? `: ${row.error_tail}` : ''}
                    </p>
                  </Show>
                  <Show when={row.status === 'running'}>
                    <div class="row-progress" aria-hidden="true">
                      <span style={{ width: `${Math.round((state()?.fraction ?? 0) * 100)}%` }} />
                    </div>
                    <Show when={state()?.stage}>
                      <p class="history-stage">{state()?.stage}</p>
                    </Show>
                  </Show>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>
    </section>
  );
}
