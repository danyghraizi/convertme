import { For, Show, createEffect, createSignal } from 'solid-js';
import ColourPicker from './colour-picker';

/**
 * The gradient editor: a strip that shows the result, the type, the angle and the
 * colour stops with their positions. The value is a single string so the plan, the
 * history row and the CLI carry exactly what the dialog shows:
 *
 *   linear|135|#0b1020@0,#4b5563@35,#fbbf24@100
 *
 * Stops are edited by identity, not by index: the list is sorted for drawing, and
 * sorting while dragging made every handle move a different stop.
 */

const MAX_STOPS = 8;
const MIN_STOPS = 2;

interface Stop {
  /** Stable within the dialog; never serialised. */
  id: number;
  colour: string;
  position: number;
}

interface GradientValue {
  kind: 'linear' | 'radial';
  angle: number;
  stops: Stop[];
}

let nextId = 1;

function parse(value: string): GradientValue {
  const text = (value ?? '').trim();
  const parts = text ? text.split('|') : [];
  const stopsText = parts.length > 1 ? parts[parts.length - 1] : parts[0] ?? '';
  let kind: 'linear' | 'radial' = 'linear';
  let angle = 0;
  for (const head of parts.slice(0, -1)) {
    const part = head.trim();
    if (!part) continue;
    if (part === 'linear' || part === 'radial') kind = part;
    else if (Number.isFinite(Number(part))) angle = Number(part);
  }
  const raw = stopsText
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => {
      const [colour, position] = entry.split('@');
      return { colour: (colour || '#000000').trim(), position };
    });
  const stops: Stop[] =
    raw.length >= MIN_STOPS
      ? raw.map((entry, index) => ({
          id: nextId++,
          colour: entry.colour,
          position:
            entry.position === undefined || !Number.isFinite(Number(entry.position))
              ? Math.round((index * 100) / (raw.length - 1))
              : Math.min(100, Math.max(0, Math.round(Number(entry.position)))),
        }))
      : [
          { id: nextId++, colour: '#f6c26b', position: 0 },
          { id: nextId++, colour: '#e4572e', position: 100 },
        ];
  return { kind, angle, stops };
}

function serialize(value: GradientValue): string {
  const stops = [...value.stops]
    .sort((a, b) => a.position - b.position)
    .map((stop) => `${stop.colour}@${Math.round(stop.position)}`)
    .join(',');
  return `${value.kind}|${Math.round(value.angle)}|${stops}`;
}

function css(value: GradientValue): string {
  const stops = [...value.stops]
    .sort((a, b) => a.position - b.position)
    .map((stop) => `${stop.colour} ${Math.round(stop.position)}%`)
    .join(', ');
  return value.kind === 'radial'
    ? `radial-gradient(circle at 50% 50%, ${stops})`
    : `linear-gradient(${Math.round(value.angle)}deg, ${stops})`;
}

/** Colour at a position, so a new stop starts from what the strip already shows. */
function colourAt(stops: Stop[], position: number): string {
  const sorted = [...stops].sort((a, b) => a.position - b.position);
  const hex = (value: string): [number, number, number] => {
    const clean = value.replace('#', '').padEnd(6, '0');
    const n = Number.parseInt(clean.slice(0, 6), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  };
  if (position <= sorted[0].position) return sorted[0].colour;
  for (let i = 1; i < sorted.length; i += 1) {
    const to = sorted[i];
    const from = sorted[i - 1];
    if (position <= to.position) {
      const span = Math.max(1, to.position - from.position);
      const k = (position - from.position) / span;
      const [r1, g1, b1] = hex(from.colour);
      const [r2, g2, b2] = hex(to.colour);
      const mix = (a: number, b: number): string =>
        Math.round(a + (b - a) * k)
          .toString(16)
          .padStart(2, '0');
      return `#${mix(r1, r2)}${mix(g1, g2)}${mix(b1, b2)}`;
    }
  }
  return sorted[sorted.length - 1].colour;
}

interface GradientEditorProps {
  value: string;
  onChange: (value: string) => void;
}

export default function GradientEditor(props: GradientEditorProps) {
  // Local list: the prop is a string, and re-parsing it on every input would mint new
  // identities (and re-sort the rows) under the pointer.
  const [state, setState] = createSignal<GradientValue>(parse(props.value));
  const [emitted, setEmitted] = createSignal(props.value);

  createEffect(() => {
    const incoming = props.value;
    if (incoming !== emitted()) {
      setState(parse(incoming));
      setEmitted(incoming);
    }
  });

  const commit = (next: GradientValue): void => {
    setState(next);
    const text = serialize(next);
    setEmitted(text);
    props.onChange(text);
  };

  const patch = (id: number, change: Partial<Stop>): void => {
    const value = state();
    commit({
      ...value,
      stops: value.stops.map((stop) => (stop.id === id ? { ...stop, ...change } : stop)),
    });
  };

  return (
    <div class="gradient-editor">
      <div class="gradient-strip" style={{ background: css(state()) }} />

      <div class="gradient-row">
        <span class="segmented" role="group" aria-label="Gradient type">
          <For each={['linear', 'radial'] as const}>
            {(kind) => (
              <button
                type="button"
                classList={{ 'is-active': state().kind === kind }}
                aria-pressed={state().kind === kind}
                onClick={() => commit({ ...state(), kind })}
              >
                {kind === 'linear' ? 'Linear' : 'Radial'}
              </button>
            )}
          </For>
        </span>
        <Show when={state().kind === 'linear'}>
          <label class="angle-row">
            <span>Angle</span>
            <input
              type="range"
              min="0"
              max="360"
              step="1"
              value={state().angle}
              onInput={(event) => commit({ ...state(), angle: Number(event.currentTarget.value) })}
            />
            <output>{Math.round(state().angle)}°</output>
          </label>
        </Show>
      </div>

      <div class="stops">
        <For each={state().stops}>
          {(stop) => (
            <div class="stop-row">
              <ColourPicker
                compact
                value={stop.colour}
                label="Colour"
                onChange={(colour) => patch(stop.id, { colour })}
              />
              <input
                type="range"
                min="0"
                max="100"
                step="1"
                value={Math.round(stop.position)}
                aria-label={`Position ${Math.round(stop.position)}%`}
                onInput={(event) =>
                  patch(stop.id, { position: Number(event.currentTarget.value) })
                }
              />
              <output>{Math.round(stop.position)}%</output>
              <button
                type="button"
                class="stop-remove"
                aria-label="Remove this colour"
                disabled={state().stops.length <= MIN_STOPS}
                onClick={() =>
                  commit({ ...state(), stops: state().stops.filter((s) => s.id !== stop.id) })
                }
              >
                ×
              </button>
            </div>
          )}
        </For>
        <button
          type="button"
          class="stop-add"
          disabled={state().stops.length >= MAX_STOPS}
          onClick={() => {
            const value = state();
            const sorted = [...value.stops].sort((a, b) => a.position - b.position);
            // In the widest gap, coloured with what the strip already shows there.
            let gapFrom = sorted[0];
            let gapTo = sorted[1];
            for (let i = 1; i < sorted.length; i += 1) {
              if (sorted[i].position - sorted[i - 1].position > gapTo.position - gapFrom.position) {
                gapFrom = sorted[i - 1];
                gapTo = sorted[i];
              }
            }
            const position = Math.round((gapFrom.position + gapTo.position) / 2);
            commit({
              ...value,
              stops: [
                ...value.stops,
                { id: nextId++, colour: colourAt(value.stops, position), position },
              ],
            });
          }}
        >
          + Add a colour
        </button>
        <span class="field-help">{MAX_STOPS} colours at most.</span>
      </div>
    </div>
  );
}
