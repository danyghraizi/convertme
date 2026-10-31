import { For, Show, createEffect, createSignal, onCleanup } from 'solid-js';
import * as ipc from '../ipc';
import { formatTimecode } from '../format';
import type { WaveformData } from '../types';
import './waveform-timeline.css';

/** A range on the waveform: a trim selection, a bleep, or a channel's own row. */
export interface WaveRange {
  start: number;
  end: number;
}

interface WaveformTimelineProps {
  path: string | null;
  duration: number;
  /** Where a click adds a range; absent means the timeline is not editable. */
  onRanges: (ranges: WaveRange[]) => void;
  ranges: WaveRange[];
  mode: 'trim' | 'bleep' | 'channels';
  /** Read-only overlays: split points, marker positions. */
  markers?: number[];
  /** The trim dialog's selected span, with its two handles. */
  range?: WaveRange;
  onRange?: (start: number, end: number) => void;
  /** Optional second channel row, drawn under the first. */
  channels?: { left: WaveformData; right: WaveformData } | null;
  /** Gains for the channel rows, as a percentage of the source (0..200). */
  gainLeft?: number;
  gainRight?: number;
  /** Hide the transport where the dialog has its own Play Output row. */
  transport?: boolean;
}

const BUCKETS = 900;

/**
 * The waveform editor: peaks, a range overlay and a playhead.
 *
 * Peaks are downsampled to the rendered width before they reach the SVG. A 400-bucket
 * waveform drawn into 900 columns would tear; a 4000-bucket one into 300 columns would
 * cost a path with thousands of segments for no visible gain.
 */
export default function WaveformTimeline(props: WaveformTimelineProps) {
  const [data, setData] = createSignal<WaveformData | null>(null);
  const [error, setError] = createSignal('');
  const [drag, setDrag] = createSignal<{ anchor: number; current: number } | null>(null);
  const [playing, setPlaying] = createSignal(false);
  const [playhead, setPlayhead] = createSignal(0);
  const [failedPlayback, setFailedPlayback] = createSignal(false);
  /** Mounted on Play: a <audio> on open initialises GStreamer for nothing. */
  const [playback, setPlayback] = createSignal(false);
  let surface: HTMLDivElement | undefined;
  let audio: HTMLAudioElement | undefined;

  // Keyed by path: a new file (or the mock's switch) reloads its own peaks.
  createEffect(() => {
    const path = props.path;
    if (!path) return;
    setData(null);
    setPlayback(false);
    void ipc
      .waveformData(path)
      .then((waveform) => setData(waveform))
      .catch((reason) => setError(reason instanceof Error ? reason.message : 'No waveform'));
  });

  const duration = (): number => {
    const fromData = data()?.duration_ms ? data()!.duration_ms / 1000 : 0;
    return props.duration > 0 ? props.duration : fromData;
  };

  function secondsAt(event: PointerEvent): number {
    const rect = surface?.getBoundingClientRect();
    if (!rect || rect.width === 0) return 0;
    const fraction = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    return fraction * duration();
  }

  function onPointerDown(event: PointerEvent): void {
    if (props.mode !== 'bleep' || duration() === 0) return;
    surface?.setPointerCapture(event.pointerId);
    const at = secondsAt(event);
    setDrag({ anchor: at, current: at });
    event.preventDefault();
  }

  function onPointerMove(event: PointerEvent): void {
    const current = drag();
    if (!current) return;
    setDrag({ anchor: current.anchor, current: secondsAt(event) });
  }

  function onPointerUp(event: PointerEvent): void {
    const current = drag();
    surface?.releasePointerCapture(event.pointerId);
    setDrag(null);
    if (!current) return;
    const start = Math.min(current.anchor, current.current);
    const end = Math.max(current.anchor, current.current);
    // A click without a drag is a click, not a one-millisecond bleep.
    if (end - start < 0.05) return;
    props.onRanges([...props.ranges, { start, end }].sort((a, b) => a.start - b.start));
  }

  function removeRange(index: number): void {
    props.onRanges(props.ranges.filter((_, position) => position !== index));
  }

  function togglePlay(): void {
    if (failedPlayback()) return;
    if (!playback()) {
      setPlayback(true);
      requestAnimationFrame(() => {
        void audio?.play().catch(() => setFailedPlayback(true));
      });
      return;
    }
    if (!audio) return;
    if (playing()) {
      audio.pause();
    } else {
      void audio.play().catch(() => setFailedPlayback(true));
    }
  }

  function setStart(value: number): void {
    const end = props.range?.end ?? duration();
    props.onRange?.(Math.min(value, Math.max(0, end - 0.01)), end);
  }

  function setEnd(value: number): void {
    const start = props.range?.start ?? 0;
    props.onRange?.(start, Math.max(value, start + 0.01));
  }

  onCleanup(() => {
    audio?.pause();
  });

  /**
   * One lane per drawn row: the peaks reduced to at most `BUCKETS` bars, scaled by the
   * row's gain, with the lane's own zero line. Computed once per data/gain change, not
   * per render of the playhead.
   */
  function lanePeaks(): { bars: { lo: number; hi: number }[]; step: number; top: number }[] {
    const build = (waveform: WaveformData | null, gain: number, top: number) => {
      if (!waveform || waveform.peaks.length === 0) return { bars: [], step: 1, top };
      const scale = Math.min(2, Math.max(0, gain / 100));
      const group = Math.max(1, Math.ceil(waveform.peaks.length / BUCKETS));
      const bars: { lo: number; hi: number }[] = [];
      for (let index = 0; index < waveform.peaks.length; index += group) {
        let lo = 0;
        let hi = 0;
        for (const [low, high] of waveform.peaks.slice(index, index + group)) {
          lo = Math.min(lo, low);
          hi = Math.max(hi, high);
        }
        bars.push({ lo: lo * scale, hi: hi * scale });
      }
      return { bars, step: 100 / bars.length, top };
    };
    if (props.channels) {
      return [
        build(props.channels.left, props.gainLeft ?? 100, 10),
        build(props.channels.right, props.gainRight ?? 100, 30),
      ];
    }
    return [build(data(), 100, 20)];
  }

  return (
    <div class="waveform-timeline">
      <div
        class="waveform-surface"
        classList={{ 'is-bleeping': props.mode === 'bleep' }}
        ref={surface}
        role="img"
        aria-label={props.mode === 'bleep' ? 'Bleep ranges' : 'Waveform'}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        <Show
          when={data()}
          fallback={<p class="waveform-empty">{error() || 'Loading waveform…'}</p>}
        >
          <svg class="waveform-svg" viewBox="0 0 100 40" preserveAspectRatio="none" aria-hidden="true">
            <For each={lanePeaks()}>
              {(lane) => (
                <>
                  <For each={lane.bars}>
                    {(bar, index) => (
                      <rect
                        x={index() * lane.step}
                        y={lane.top - bar.hi * 10}
                        width={Math.max(lane.step, 0.12)}
                        height={Math.max(0.4, (bar.hi - bar.lo) * 10)}
                        fill="currentColor"
                      />
                    )}
                  </For>
                  <line
                    x1="0"
                    x2="100"
                    y1={lane.top}
                    y2={lane.top}
                    stroke="var(--rule)"
                    stroke-width="0.2"
                  />
                </>
              )}
            </For>
          </svg>
          <For each={props.ranges}>
            {(range, index) => (
              <span
                class="waveform-range"
                classList={{ 'is-bleep': props.mode === 'bleep' }}
                style={{
                  left: `${(range.start / Math.max(duration(), 0.001)) * 100}%`,
                  width: `${((range.end - range.start) / Math.max(duration(), 0.001)) * 100}%`,
                }}
                title={`${formatTimecode(range.start)} – ${formatTimecode(range.end)}`}
              >
                <Show when={props.mode === 'bleep'}>
                  <button
                    type="button"
                    class="waveform-range-remove"
                    aria-label={`Remove bleep ${index() + 1}`}
                    onClick={() => removeRange(index())}
                  >
                    ×
                  </button>
                </Show>
              </span>
            )}
          </For>
          <Show when={drag()}>
            {(current) => {
              const start = (): number => Math.min(current().anchor, current().current);
              const end = (): number => Math.max(current().anchor, current().current);
              return (
                <span
                  class="waveform-range is-draft"
                  style={{
                    left: `${(start() / Math.max(duration(), 0.001)) * 100}%`,
                    width: `${((end() - start()) / Math.max(duration(), 0.001)) * 100}%`,
                  }}
                />
              );
            }}
          </Show>
          <For each={props.markers ?? []}>
            {(marker) => (
              <span
                class="waveform-marker"
                style={{ left: `${(marker / Math.max(duration(), 0.001)) * 100}%` }}
              />
            )}
          </For>
          <Show when={props.range && props.onRange}>
            <span
              class="waveform-selection"
              style={{
                left: `${((props.range?.start ?? 0) / Math.max(duration(), 0.001)) * 100}%`,
                width: `${
                  (((props.range?.end ?? 0) - (props.range?.start ?? 0)) /
                    Math.max(duration(), 0.001)) *
                  100
                }%`,
              }}
            />
          </Show>
          <span
            class="waveform-playhead"
            style={{ left: `${(playhead() / Math.max(duration(), 0.001)) * 100}%` }}
          />
        </Show>
        <Show when={props.channels}>
          <span class="waveform-lane-label" style={{ top: '10%' }}>
            L
          </span>
          <span class="waveform-lane-label" style={{ top: '55%' }}>
            R
          </span>
        </Show>
      </div>
      <div class="waveform-legend">
        <span class="waveform-time">0:00</span>
        <Show when={data()?.truncated}>
          <span class="waveform-note">first 10 minutes shown</span>
        </Show>
        <span class="waveform-time">{formatTimecode(duration())}</span>
      </div>
      <Show when={props.range && props.onRange}>
        <div class="waveform-handles">
          <label class="waveform-handle">
            <span class="waveform-handle-label">Trim start</span>
            <input
              type="range"
              aria-label="Trim start"
              min={0}
              max={Math.max(0, duration())}
              step={0.01}
              value={props.range?.start ?? 0}
              onInput={(event) => setStart(event.currentTarget.valueAsNumber)}
            />
          </label>
          <label class="waveform-handle">
            <span class="waveform-handle-label">Trim end</span>
            <input
              type="range"
              aria-label="Trim end"
              min={0}
              max={Math.max(0, duration())}
              step={0.01}
              value={props.range?.end ?? 0}
              onInput={(event) => setEnd(event.currentTarget.valueAsNumber)}
            />
          </label>
          <span class="waveform-selection-readout">
            {formatTimecode(props.range?.start ?? 0)} – {formatTimecode(props.range?.end ?? 0)}
          </span>
        </div>
      </Show>
      <Show when={props.transport !== false && props.path}>
        <div class="waveform-transport">
          <button
            type="button"
            class="timeline-button"
            aria-label={playing() ? 'Pause' : 'Play'}
            disabled={failedPlayback()}
            onClick={togglePlay}
          >
            {playing() ? '❚❚' : '▶'}
          </button>
          <span class="waveform-timecode">
            {formatTimecode(playhead())}
            <span class="waveform-timecode-total"> / {formatTimecode(duration())}</span>
          </span>
          <Show when={failedPlayback()}>
            <span class="waveform-note">This format cannot be played here.</span>
          </Show>
          <Show when={playback()}>
            <audio
              ref={audio}
              src={props.path ? ipc.assetUrl(props.path) : undefined}
              preload="metadata"
              class="waveform-audio"
              onTimeUpdate={(event) => setPlayhead(event.currentTarget.currentTime)}
              onPlay={() => setPlaying(true)}
              onPause={() => setPlaying(false)}
              onEnded={() => setPlaying(false)}
              onError={() => setFailedPlayback(true)}
            />
          </Show>
        </div>
      </Show>
      <Show when={props.mode === 'bleep'}>
        <p class="field-help">
          {props.ranges.length === 0
            ? 'Drag across the waveform to mark a range to bleep.'
            : `${props.ranges.length} range${props.ranges.length === 1 ? '' : 's'} will be replaced with a tone.`}
        </p>
      </Show>
    </div>
  );
}
