import { For, Show } from 'solid-js';
import ColourPicker from './colour-picker';

/**
 * The gradient editor: a strip that shows the result, the type, the angle and the
 * colour stops with their positions. The value is a single string so the plan, the
 * history row and the CLI carry exactly what the dialog shows:
 *
 *   linear|135|#0b1020@0,#4b5563@35,#fbbf24@100
 */

const MAX_STOPS = 8;

interface Stop {
  colour: string;
  position: number;
}

interface GradientValue {
  kind: 'linear' | 'radial';
  angle: number;
  stops: Stop[];
}

function parse(value: string): GradientValue {
  const fallback: GradientValue = {
    kind: 'linear',
    angle: 0,
    stops: [
      { colour: '#f6c26b', position: 0 },
      { colour: '#e4572e', position: 100 },
    ],
  };
  const text = (value ?? '').trim();
  if (!text) return fallback;
  const parts = text.split('|');
  const stopsText = parts.length > 1 ? parts[parts.length - 1] : parts[0];
  let kind: 'linear' | 'radial' = 'linear';
  let angle = 0;
  if (parts.length > 1) {
    for (const head of parts.slice(0, -1)) {
      const part = head.trim();
      if (!part) continue;
      if (part === 'linear' || part === 'radial') kind = part;
      else if (Number.isFinite(Number(part))) angle = Number(part);
    }
  }
  const raw = stopsText
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => {
      const [colour, position] = entry.split('@');
      return {
        colour: (colour || '#000000').trim(),
        position: position === undefined ? Number.NaN : Number(position),
      };
    });
  if (raw.length < 2) return fallback;
  const last = raw.length - 1;
  const stops = raw.map((stop, index) => ({
    colour: stop.colour,
    position: Number.isFinite(stop.position)
      ? Math.min(100, Math.max(0, stop.position))
      : (index * 100) / last,
  }));
  stops.sort((a, b) => a.position - b.position);
  stops[0].position = 0;
  stops[stops.length - 1].position = 100;
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

interface GradientEditorProps {
  value: string;
  onChange: (value: string) => void;
}

export default function GradientEditor(props: GradientEditorProps) {
  const busy = (): boolean => false;
  const current = (): GradientValue => parse(props.value);
  const update = (next: GradientValue): void => props.onChange(serialize(next));

  const withStop = (index: number, patch: Partial<Stop>): void => {
    const value = current();
    value.stops = value.stops.map((stop, i) => (i === index ? { ...stop, ...patch } : stop));
    update(value);
  };

  return (
    <div class="gradient-editor">
      <div class="gradient-strip" style={{ background: css(current()) }} />

      <div class="gradient-row">
        <span class="segmented" role="group" aria-label="Gradient type">
          <For each={['linear', 'radial'] as const}>
            {(kind) => (
              <button
                type="button"
                classList={{ 'is-active': current().kind === kind }}
                aria-pressed={current().kind === kind}
                onClick={() => {
                  const value = current();
                  value.kind = kind;
                  update(value);
                }}
              >
                {kind === 'linear' ? 'Linear' : 'Radial'}
              </button>
            )}
          </For>
        </span>
        <Show when={current().kind === 'linear'}>
          <label class="angle-row">
            <span>Angle</span>
            <input
              type="range"
              min="0"
              max="360"
              step="1"
              value={current().angle}
              onInput={(event) => {
                const value = current();
                value.angle = Number(event.currentTarget.value);
                update(value);
              }}
            />
            <output>{Math.round(current().angle)}°</output>
          </label>
        </Show>
      </div>

      <div class="stops">
        <For each={current().stops}>
          {(stop, index) => (
            <div class="stop-row">
              <ColourPicker
                compact
                value={stop.colour}
                label={`Colour ${index() + 1}`}
                onChange={(colour) => withStop(index(), { colour })}
              />
              <input
                type="range"
                min="0"
                max="100"
                step="1"
                value={Math.round(stop.position)}
                aria-label={`Position ${index() + 1}`}
                onInput={(event) =>
                  withStop(index(), { position: Number(event.currentTarget.value) })
                }
              />
              <output>{Math.round(stop.position)}%</output>
              <button
                type="button"
                class="stop-remove"
                aria-label="Remove this colour"
                disabled={current().stops.length <= 2 || busy()}
                onClick={() => {
                  const value = current();
                  value.stops = value.stops.filter((_, i) => i !== index());
                  update(value);
                }}
              >
                ×
              </button>
            </div>
          )}
        </For>
        <button
          type="button"
          class="stop-add"
          disabled={current().stops.length >= MAX_STOPS || busy()}
          onClick={() => {
            const value = current();
            const last = value.stops[value.stops.length - 1];
            const previous = value.stops[value.stops.length - 2] ?? last;
            value.stops = [
              ...value.stops.slice(0, -1),
              {
                colour: previous.colour,
                position: (previous.position + last.position) / 2,
              },
              last,
            ];
            update(value);
          }}
        >
          + Add a colour
        </button>
        <span class="field-help">{MAX_STOPS} colours at most.</span>
      </div>
    </div>
  );
}
