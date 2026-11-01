import { For, Show, createEffect, createMemo, createSignal, onCleanup } from 'solid-js';
import * as ipc from '../ipc';
import { asNumber } from '../fields';
import { formatTimecode } from '../format';
import { PauseGlyph, PlayGlyph } from './play-glyph';
import WaveformTimeline, { type WaveRange } from './waveform-timeline';
import './audio-editor.css';
import type { FieldValue, MediaInfo } from '../types';

interface AudioEditorProps {
  tool: string;
  path: string | null;
  values: Record<string, FieldValue>;
  setValue: (key: string, value: FieldValue) => void;
  /** The current field values, so a preview runs the same parameters as the job. */
  collect: () => Record<string, unknown>;
  onExtras: (extras: Record<string, unknown>) => void;
  onCount: (count: number) => void;
  onReady: (ready: boolean) => void;
  /** Bumped by the footer's Reset; the bleep lane clears with the rest. */
  resetToken: number;
}

/** The still frame of the visualizer, drawn from the same peaks the job animates. */
const PREVIEW_BUCKETS = 140;

const round = (value: number): number => Math.round(value * 1000) / 1000;

/**
 * Relative luminance of a `#rgb`/`#rrggbb` colour (WCAG's formula), so the wave ink
 * stays readable whatever Background the user picks. Unparseable values are treated
 * as dark: a white wave is the safe drawing on a named dark colour.
 */
function relativeLuminance(color: string): number {
  let hex = color.trim().replace(/^#/, '');
  if (hex.length === 3) hex = hex.replace(/./g, (digit) => digit + digit);
  if (!/^[0-9a-f]{6}$/i.test(hex)) return 0;
  const channel = (offset: number): number => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
}

/**
 * The audio dialog's surfaces: the waveform and its trim handles, the two channel
 * rows, the measured loudness of a normalize target, the bleep lane and the
 * visualizer's still. A preview only runs when its own button is pressed.
 */
export default function AudioEditor(props: AudioEditorProps) {
  const [info, setInfo] = createSignal<MediaInfo | null>(null);
  const [ranges, setRanges] = createSignal<WaveRange[]>([]);
  const [channels, setChannels] = createSignal<{
    left: { duration_ms: number; truncated: boolean; peaks: [number, number][] };
    right: { duration_ms: number; truncated: boolean; peaks: [number, number][] };
  } | null>(null);
  const [peaks, setPeaks] = createSignal<[number, number][]>([]);
  const [measured, setMeasured] = createSignal<number | null>(null);
  const [previewReady, setPreviewReady] = createSignal('');
  const [previewPlaying, setPreviewPlaying] = createSignal(false);
  const [previewLabel, setPreviewLabel] = createSignal('');
  const [previewBusy, setPreviewBusy] = createSignal(false);
  const [previewError, setPreviewError] = createSignal('');
  const [previewPosition, setPreviewPosition] = createSignal(0);
  /** The bridge rejects with a plain string; that sentence is the whole message. */
  const rejection = (cause: unknown): string =>
    typeof cause === 'string'
      ? cause
      : cause instanceof Error
        ? cause.message
        : 'The preview failed';
  // Playback is one shared external sink: the web view never decodes audio. The sink
  // is started at a position and stopped by `stop_preview`; this dialog advances its
  // own clock, so Pause resumes exactly where it stopped and a trim stops at its end.
  let previewTimer: number | undefined;
  let previewStarted = 0;
  let previewFrom = 0;
  let previewStopAt = 0;
  /** A render that finishes after another preview was started must not play. */
  let previewRequest = 0;

  function clearPreviewClock(): void {
    if (previewTimer !== undefined) {
      window.clearInterval(previewTimer);
      previewTimer = undefined;
    }
  }

  function stopPreview(): void {
    previewRequest += 1;
    clearPreviewClock();
    setPreviewPlaying(false);
    void ipc.stopPreview();
  }

  /** A click on the waveform seeks the produced preview (not just a marker). */
  function seekPreview(seconds: number): void {
    if (previewPlaying()) stopPreview();
    setPreviewPosition(Math.min(duration(), Math.max(0, seconds)));
  }

  function startPreviewClock(from: number, stopAt: number): void {
    previewFrom = from;
    previewStopAt = stopAt;
    previewStarted = performance.now();
    setPreviewPosition(from);
    clearPreviewClock();
    previewTimer = window.setInterval(() => {
      const next = previewFrom + (performance.now() - previewStarted) / 1000;
      if (next >= previewStopAt) {
        setPreviewPosition(previewStopAt);
        stopPreview();
        return;
      }
      setPreviewPosition(next);
    }, 100);
  }

  /** A label is playing when the sink and the button's own label agree. */
  const isPlaying = (label: string): boolean =>
    previewPlaying() && previewLabel() === label;

  async function play(path: string, label: string, start: number, stopAt: number): Promise<void> {
    setPreviewLabel(label);
    setPreviewError('');
    setPreviewPlaying(true);
    try {
      await ipc.playPreview(path, start);
      startPreviewClock(start, stopAt);
    } catch (cause) {
      setPreviewError(rejection(cause));
      setPreviewPlaying(false);
    }
  }

  /**
   * One button per version. Pressing the active label pauses; pressing another label
   * switches. The sink is shared, so the previous player is stopped either way.
   */
  function toggle(path: string | null, label: string, start = 0, stopAt = duration()): void {
    if (!path) return;
    if (isPlaying(label)) {
      stopPreview();
      return;
    }
    previewRequest += 1;
    // The position is shared by every version: switching Original ↔ With Bleeps at
    // 0:07 must continue at 0:07, not jump back to the start.
    const from = previewPosition() < stopAt - 0.05 ? previewPosition() : start;
    void play(path, label, from, stopAt);
  }

  onCleanup(stopPreview);

  const duration = (): number => info()?.duration ?? 0;
  const start = (): number => asNumber(props.values.start ?? 0);
  const end = (): number => asNumber(props.values.end ?? 0);
  /** The chosen output layout, and whether the source itself is mono. */
  const monoOutput = (): boolean => String(props.values.channels ?? '2') === '1';
  const sourceMono = (): boolean => (info()?.channels ?? 0) === 1;
  const gainLeft = (): number =>
    asNumber(monoOutput() ? (props.values.volume ?? 100) : (props.values.left_volume ?? 100));
  const gainRight = (): number => asNumber(props.values.right_volume ?? 100);
  /** A mono source feeding stereo is the same signal on both rows, as the engine does. */
  const laneData = () => {
    const data = channels();
    if (!data) return null;
    if (sourceMono() && !monoOutput()) return { left: data.left, right: data.left };
    return data;
  };

  const extras = (): Record<string, unknown> =>
    props.tool === 'bleep' && ranges().length > 0 ? { ranges: ranges() } : {};

  /** Preview parameters = the dialog's collected fields + the editor's injected ones. */
  const paramsFor = (): Record<string, unknown> => ({ ...props.collect(), ...extras() });

  createEffect(() => {
    const path = props.path;
    if (!path) return;
    setInfo(null);
    setRanges([]);
    setChannels(null);
    setPeaks([]);
    setMeasured(null);
    setPreviewReady('');
    stopPreview();
    setPreviewPosition(0);
    setPreviewError('');
    void ipc
      .mediaInfo(path)
      .then((facts) => setInfo(facts))
      .catch(() => setInfo(null));
  });

  createEffect(() => {
    props.onExtras({});
    props.onCount(0);
    props.onReady(true);
  });

  createEffect(() => {
    if (props.resetToken < 0) return;
    setRanges([]);
    setPreviewReady('');
    stopPreview();
    setPreviewPosition(0);
    setPreviewError('');
  });

  createEffect(() => {
    if (props.tool !== 'trim') return;
    const media = info();
    if (!media || media.duration <= 0) return;
    if (end() <= start() || end() > media.duration + 0.01) {
      props.setValue('start', 0);
      props.setValue('end', round(media.duration));
    }
  });

  // Bleep: the lane's ranges are the value.
  createEffect(() => {
    if (props.tool !== 'bleep') return;
    const current = ranges();
    props.onReady(current.length > 0);
    props.onExtras(current.length > 0 ? { ranges: current } : {});
    props.onCount(current.length);
  });

  // Channels: both source rows, with the dialog's gains applied to the drawing.
  createEffect(() => {
    if (props.tool !== 'channels') return;
    const path = props.path;
    if (!path) return;
    void ipc
      .channelWaveforms(path)
      .then((data) => setChannels(data))
      .catch(() => setChannels(null));
  });

  // Normalize shows what the file measures before the job runs.
  createEffect(() => {
    if (props.tool !== 'normalize') return;
    const path = props.path;
    if (!path) return;
    void ipc
      .measureLoudness(path)
      .then((loudness) => setMeasured(loudness.input_i))
      .catch(() => setMeasured(null));
  });

  // The visualizer's still: the same waveform buckets the MP4 animates.
  createEffect(() => {
    if (props.tool !== 'visualize') return;
    const path = props.path;
    if (!path) return;
    void ipc
      .waveformData(path)
      .then((data) => setPeaks(data.peaks))
      .catch(() => setPeaks([]));
  });

  const shapeRatio = createMemo(() => {
    const shape = String(props.values.shape ?? '16:9');
    if (shape === '9:16') return '9 / 16';
    if (shape === '1:1') return '1 / 1';
    return '16 / 9';
  });

  // The frame carries the job's Background; the wave (currentColor in the SVG) needs
  // the opposite ink to stay visible on that background.
  const previewBackground = createMemo(() => String(props.values.color ?? '#101014'));
  const previewInk = createMemo(() =>
    relativeLuminance(previewBackground()) > 0.4 ? '#141416' : '#ffffff',
  );

  /** Peaks reduced to the drawing width, keeping each bucket's larger side. */
  const previewBars = createMemo(() => {
    const all = peaks();
    if (all.length === 0) return [] as { lo: number; hi: number }[];
    const group = Math.max(1, Math.ceil(all.length / PREVIEW_BUCKETS));
    const bars: { lo: number; hi: number }[] = [];
    for (let index = 0; index < all.length; index += group) {
      let lo = 0;
      let hi = 0;
      for (const [low, high] of all.slice(index, index + group)) {
        lo = Math.min(lo, low);
        hi = Math.max(hi, high);
      }
      bars.push({ lo, hi });
    }
    return bars;
  });

  const style = (): string => String(props.values.style ?? 'waveform');

  async function playOutput(tool: string): Promise<void> {
    const path = props.path;
    if (!path || previewBusy()) return;
    if (isPlaying('Output')) {
      stopPreview();
      return;
    }
    const token = ++previewRequest;
    setPreviewBusy(true);
    setPreviewReady('');
    setPreviewError('');
    try {
      const produced = await ipc.audioPreview(path, tool, paramsFor());
      if (token !== previewRequest) return;
      setPreviewReady('Output');
      const from = previewPosition() < duration() - 0.05 ? previewPosition() : 0;
      await play(produced, 'Output', from, duration());
    } catch (cause) {
      setPreviewError(rejection(cause));
    } finally {
      setPreviewBusy(false);
    }
  }

  async function playBleeps(): Promise<void> {
    const path = props.path;
    if (!path || previewBusy()) return;
    if (isPlaying('With Bleeps')) {
      stopPreview();
      return;
    }
    const token = ++previewRequest;
    setPreviewBusy(true);
    setPreviewReady('');
    setPreviewError('');
    try {
      const produced = await ipc.bleepPreview(path, paramsFor());
      if (token !== previewRequest) return;
      setPreviewReady('With Bleeps');
      // The bleeped copy has the same length as the source; the clock ends there.
      const from = previewPosition() < duration() - 0.05 ? previewPosition() : 0;
      await play(produced, 'With Bleeps', from, duration());
    } catch (cause) {
      setPreviewError(rejection(cause));
    } finally {
      setPreviewBusy(false);
    }
  }

  return (
    <div class="audio-editor">
      <Show when={props.tool === 'trim'}>
        <div class="field is-block">
          <span class="field-label">Waveform</span>
          <WaveformTimeline
            path={props.path}
            duration={duration()}
            mode="trim"
            ranges={[]}
            onRanges={() => undefined}
            range={{ start: start(), end: end() }}
            onRange={(nextStart, nextEnd) => {
              props.setValue('start', round(nextStart));
              props.setValue('end', round(nextEnd));
            }}
          />
        </div>
      </Show>

      <Show when={props.tool === 'normalize'}>
        <div class="field is-block">
          <span class="field-label">Loudness</span>
          <WaveformTimeline
            path={props.path}
            duration={duration()}
            mode="trim"
            ranges={[]}
            onRanges={() => undefined}
            transport={false}
            playhead={previewPosition()}
            onSeek={seekPreview}
          />
          <div class="audio-preview-row">
            <button
              type="button"
              class="button button-quiet"
              disabled={previewBusy()}
              onClick={() => void playOutput('normalize')}
            >
              <Show when={previewBusy()} fallback={
                <>
                  <Show when={isPlaying('Output')} fallback={<PlayGlyph />}>
                    <PauseGlyph />
                  </Show>
                  Output
                </>
              }>
                Rendering…
              </Show>
            </button>
            <span class="audio-measured">
              {measured() === null
                ? 'Measuring the input…'
                : `Input: ${measured()!.toFixed(1)} LUFS`}
            </span>
          </div>
          <Show when={previewReady()}>
            <p class="field-help" role="status">
              Preview ready.
            </p>
          </Show>
          <Show when={previewError()}>
            <p class="field-help" role="status">
              {previewError()}
            </p>
          </Show>
        </div>
      </Show>

      <Show when={props.tool === 'channels'}>
        <div class="field is-block">
          <span class="field-label">Channels</span>
          <WaveformTimeline
            path={props.path}
            duration={duration()}
            mode="channels"
            channels={laneData()}
            mono={monoOutput()}
            gainLeft={gainLeft()}
            gainRight={gainRight()}
            ranges={[]}
            onRanges={() => undefined}
            transport={false}
            playhead={previewPosition()}
            onSeek={seekPreview}
          />
          <div class="audio-preview-row">
            <button
              type="button"
              class="button button-quiet"
              disabled={previewBusy()}
              onClick={() => void playOutput('channels')}
            >
              <Show when={previewBusy()} fallback={
                <>
                  <Show when={isPlaying('Output')} fallback={<PlayGlyph />}>
                    <PauseGlyph />
                  </Show>
                  Output
                </>
              }>
                Rendering…
              </Show>
            </button>
            <Show when={previewReady()}>
              <p class="field-help" role="status">
                Preview ready.
              </p>
            </Show>
          </div>
          <Show when={previewError()}>
            <p class="field-help" role="status">
              {previewError()}
            </p>
          </Show>
        </div>
      </Show>

      <Show when={props.tool === 'visualize'}>
        <div class="field is-block">
          <span class="field-label">Preview</span>
          <div
            class="visualizer-frame"
            classList={{
              'is-portrait': String(props.values.shape ?? '16:9') === '9:16',
              'is-square': String(props.values.shape ?? '16:9') === '1:1',
            }}
            style={{
              'aspect-ratio': shapeRatio(),
              background: previewBackground(),
              color: previewInk(),
            }}
          >
            <Show
              when={previewBars().length > 0}
              fallback={<span class="visualizer-empty">Loading the waveform…</span>}
            >
              <svg
                class="visualizer-svg"
                viewBox="0 0 100 100"
                preserveAspectRatio="none"
                aria-label="Visualizer preview"
              >
                <Show when={style() === 'line'}>
                  <polyline
                    points={previewBars()
                      .map((bar, index) => {
                        const x = (index / Math.max(1, previewBars().length - 1)) * 100;
                        return `${x.toFixed(2)},${(50 - bar.hi * 45).toFixed(2)}`;
                      })
                      .join(' ')}
                    fill="none"
                    stroke="currentColor"
                    stroke-width="0.8"
                  />
                </Show>
                <Show when={style() !== 'line'}>
                  <For each={previewBars()}>
                    {(bar, index) => (
                      <rect
                        x={(index() / previewBars().length) * 100}
                        y={style() === 'bars' ? 50 - bar.hi * 45 : 50 - bar.hi * 45}
                        width={Math.max(100 / previewBars().length, 0.2)}
                        height={
                          style() === 'bars'
                            ? Math.max(0.6, bar.hi * 45)
                            : Math.max(0.6, (bar.hi - bar.lo) * 45)
                        }
                        fill="currentColor"
                      />
                    )}
                  </For>
                </Show>
              </svg>
            </Show>
          </div>
          <div class="audio-preview-row">
            <button
              type="button"
              class="button button-quiet"
              onClick={() => toggle(props.path, 'Audio')}
            >
              <Show when={isPlaying('Audio')} fallback={<PlayGlyph />}>
                <PauseGlyph />
              </Show>
              Audio
            </button>
            <span class="audio-measured">
              {previewPlaying() && previewLabel() === 'Audio'
                ? `${formatTimecode(previewPosition())} / ${formatTimecode(duration())}`
                : ''}
            </span>
          </div>
        </div>
      </Show>

      <Show when={props.tool === 'bleep'}>
        <div class="field is-block">
          <span class="field-label">Bleep ranges</span>
          <WaveformTimeline
            path={props.path}
            duration={duration()}
            mode="bleep"
            ranges={ranges()}
            onRanges={setRanges}
            transport={false}
            playhead={previewPosition()}
            onSeek={seekPreview}
          />
          <div class="audio-preview-row">
            <button
              type="button"
              class="button button-quiet"
              onClick={() => toggle(props.path, 'Original')}
            >
              <Show when={isPlaying('Original')} fallback={<PlayGlyph />}>
                <PauseGlyph />
              </Show>
              Original
            </button>
            <button
              type="button"
              class="button button-quiet"
              disabled={previewBusy() || ranges().length === 0}
              onClick={() => void playBleeps()}
            >
              <Show
                when={!previewBusy()}
                fallback={<>Rendering…</>}
              >
                <Show when={isPlaying('With Bleeps')} fallback={<PlayGlyph />}>
                  <PauseGlyph />
                </Show>
                With Bleeps
              </Show>
            </button>
            <span class="audio-measured">
              {previewPlaying()
                ? `${previewLabel()} · ${formatTimecode(previewPosition())} / ${formatTimecode(duration())}`
                : previewLabel()}
            </span>
          </div>
          <Show when={previewReady()}>
            <p class="field-help" role="status">
              {previewReady()} ready.
            </p>
          </Show>
          <Show when={previewError()}>
            <p class="field-help" role="status">
              {previewError()}
            </p>
          </Show>
        </div>
      </Show>
    </div>
  );
}
