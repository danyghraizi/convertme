import { For, Show } from 'solid-js';
import { formatTimecode } from '../format';
import './range-chips.css';

interface RangeChipsProps {
  /** Selected times, in seconds, ascending and unique. */
  times: number[];
  onChange: (times: number[]) => void;
  /** Where "Add frame" takes its value from. */
  playhead: number;
  /** The engine's own cap (e.g. 200 frames per job). */
  max?: number;
  addLabel?: string;
  emptyHelp?: string;
}

/** Two selected times are the same frame below this distance. */
const EPSILON = 0.01;

/**
 * The chips of chosen moments — frames to save, or the points a clip is cut at.
 *
 * The list is the value: adding sorts a copy, removing filters by index of the sorted
 * list, and the source array is never edited in place (lesson 13: an editable list
 * that re-sorts under the pointer loses the row the user grabbed).
 */
export default function RangeChips(props: RangeChipsProps) {
  function add(): void {
    const at = props.playhead;
    if (at < 0) return;
    if (props.times.some((time) => Math.abs(time - at) < EPSILON)) return;
    if (props.max !== undefined && props.times.length >= props.max) return;
    props.onChange([...props.times, at].sort((a, b) => a - b));
  }

  function drop(index: number): void {
    props.onChange(props.times.filter((_, position) => position !== index));
  }

  return (
    <div class="range-chips">
      <div class="range-chip-row">
        <button
          type="button"
          class="range-chip-add"
          onClick={add}
          disabled={props.max !== undefined && props.times.length >= props.max}
        >
          {props.addLabel ?? 'Add frame'}
        </button>
        <For each={props.times}>
          {(time, index) => (
            <span class="range-chip">
              <span class="range-chip-time">{formatTimecode(time)}</span>
              <button
                type="button"
                class="range-chip-remove"
                aria-label={`Remove frame ${formatTimecode(time)}`}
                onClick={() => drop(index())}
              >
                ×
              </button>
            </span>
          )}
        </For>
      </div>
      <Show when={props.times.length === 0}>
        <p class="field-help">{props.emptyHelp ?? 'Step to a moment and add it.'}</p>
      </Show>
      <Show when={props.max !== undefined && props.times.length >= props.max}>
        <p class="field-help" role="status">
          {props.max} is the most this tool writes in one go.
        </p>
      </Show>
    </div>
  );
}
