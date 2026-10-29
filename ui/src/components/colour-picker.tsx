import { Show, createEffect, createSignal, onCleanup, onMount } from 'solid-js';

/**
 * A colour picker with a saturation/brightness square, a hue strip, the hex value and
 * the presets. It is an *additional* way to pick: the native picker button and the
 * swatches stay, because the fastest path to "that colour" differs per person.
 */
interface ColourPickerProps {
  value: string;
  onChange: (value: string) => void;
  /** Stops use the square only: their row is already dense. */
  compact?: boolean;
  label?: string;
}

const SWATCHES = [
  '#101418',
  '#ffffff',
  '#9aa5b1',
  '#f6c26b',
  '#e4572e',
  '#2f855a',
  '#2b6cb0',
  '#6b46c1',
];

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

function hexToRgb(hex: string): [number, number, number] {
  const clean = (hex || '').trim().replace('#', '');
  const full =
    clean.length === 3
      ? clean
          .split('')
          .map((c) => c + c)
          .join('')
      : clean.padEnd(6, '0').slice(0, 6);
  const n = Number.parseInt(full, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function rgbToHex(rgb: [number, number, number]): string {
  return `#${rgb.map((c) => Math.round(clamp01(c / 255) * 255).toString(16).padStart(2, '0')).join('')}`;
}

function rgbToHsv([r, g, b]: [number, number, number]): [number, number, number] {
  const [rr, gg, bb] = [r / 255, g / 255, b / 255];
  const max = Math.max(rr, gg, bb);
  const min = Math.min(rr, gg, bb);
  const delta = max - min;
  let hue = 0;
  if (delta > 0) {
    if (max === rr) hue = 60 * (((gg - bb) / delta) % 6);
    else if (max === gg) hue = 60 * ((bb - rr) / delta + 2);
    else hue = 60 * ((rr - gg) / delta + 4);
  }
  return [(hue + 360) % 360, max === 0 ? 0 : delta / max, max];
}

function hsvToHex(h: number, s: number, v: number): string {
  const c = v * s;
  const hp = (h % 360) / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  const [r1, g1, b1] =
    hp < 1
      ? [c, x, 0]
      : hp < 2
        ? [x, c, 0]
        : hp < 3
          ? [0, c, x]
          : hp < 4
            ? [0, x, c]
            : hp < 5
              ? [x, 0, c]
              : [c, 0, x];
  const m = v - c;
  return rgbToHex([(r1 + m) * 255, (g1 + m) * 255, (b1 + m) * 255]);
}

export default function ColourPicker(props: ColourPickerProps) {
  const [open, setOpen] = createSignal(false);
  const [hsv, setHsv] = createSignal<[number, number, number]>(rgbToHsv(hexToRgb(props.value)));
  const [hex, setHex] = createSignal(props.value);

  // The value can change from outside (a preset, a stop reset, "Reset").
  createEffect(() => {
    const value = props.value;
    setHex(value);
    const [h, s, v] = rgbToHsv(hexToRgb(value));
    setHsv((current) => (current[1] === 0 && s === 0 ? [h, s, v] : [h, s, v]));
  });

  const emit = (h: number, s: number, v: number): void => {
    setHsv([h, s, v]);
    const next = hsvToHex(h, s, v);
    setHex(next);
    props.onChange(next);
  };

  onMount(() => {
    const onDown = (event: MouseEvent): void => {
      if (!open()) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest('.colour-popover') || target?.closest('.colour-trigger')) return;
      setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    onCleanup(() => document.removeEventListener('mousedown', onDown));
  });

  const drag = (event: PointerEvent, element: HTMLElement, onMove: (x: number, y: number) => void): void => {
    element.setPointerCapture(event.pointerId);
    const apply = (e: PointerEvent): void => {
      const rect = element.getBoundingClientRect();
      const x = clamp01((e.clientX - rect.left) / Math.max(1, rect.width));
      const y = clamp01((e.clientY - rect.top) / Math.max(1, rect.height));
      onMove(x, y);
    };
    apply(event);
    const move = (e: PointerEvent): void => apply(e);
    const up = (): void => {
      element.removeEventListener('pointermove', move);
      element.removeEventListener('pointerup', up);
      element.removeEventListener('pointercancel', up);
    };
    element.addEventListener('pointermove', move);
    element.addEventListener('pointerup', up);
    element.addEventListener('pointercancel', up);
  };

  const square = (): string => {
    const [h] = hsv();
    return `linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, transparent), hsl(${h} 100% 50%)`;
  };

  return (
    <div class="colour-control">
      <button
        type="button"
        class="colour-trigger"
        aria-label={props.label ?? 'Colour'}
        style={{ background: props.value }}
        onClick={() => setOpen((v) => !v)}
      />
      <Show when={!props.compact}>
        <span class="swatches" role="group" aria-label="Presets">
          {SWATCHES.map((colour) => (
            <button
              type="button"
              class="swatch"
              classList={{
                'is-active': (props.value || '').toLowerCase() === colour,
              }}
              style={{ background: colour }}
              aria-label={colour}
              title={colour}
              onClick={() => props.onChange(colour)}
            />
          ))}
        </span>
      </Show>
      <input
        class="colour-hex"
        type="text"
        value={hex()}
        spellcheck={false}
        onChange={(event) => {
          const next = event.currentTarget.value;
          setHex(next);
          props.onChange(next);
        }}
      />

      <Show when={open()}>
        <div class="colour-popover" role="dialog" aria-label="Pick a colour">
          <div
            class="sv-square"
            style={{ background: square() }}
            onPointerDown={(event) => {
              const target = event.currentTarget;
              drag(event, target, (x, y) => {
                const [h] = hsv();
                emit(h, x, 1 - y);
              });
            }}
          >
            <span
              class="sv-knob"
              style={{
                left: `${hsv()[1] * 100}%`,
                top: `${(1 - hsv()[2]) * 100}%`,
                background: props.value,
              }}
            />
          </div>
          <div class="hue-row">
            <div
              class="hue-bar"
              onPointerDown={(event) => {
                const target = event.currentTarget;
                drag(event, target, (x) => {
                  const [, s, v] = hsv();
                  emit(Math.round(x * 359), s, v);
                });
              }}
            >
              <span class="hue-knob" style={{ left: `${(hsv()[0] / 360) * 100}%` }} />
            </div>
            <input
              type="number"
              min="0"
              max="360"
              value={Math.round(hsv()[0])}
              onChange={(event) => {
                const [, s, v] = hsv();
                emit(Number(event.currentTarget.value) || 0, s, v);
              }}
            />
          </div>
          <label class="hex-row">
            <span>Hex</span>
            <input
              type="text"
              value={hex()}
              spellcheck={false}
              onInput={(event) => {
                const next = event.currentTarget.value;
                setHex(next);
                props.onChange(next);
              }}
            />
          </label>
        </div>
      </Show>
    </div>
  );
}
