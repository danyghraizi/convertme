import { For, Show, createEffect, createMemo, createSignal, onCleanup } from 'solid-js';
import * as ipc from '../ipc';
import { formatTimecode } from '../format';
import './media-timeline.css';

interface MediaTimelineProps {
  path: string | null;
  /** Clip length in seconds, from `media_info`. */
  duration: number;
  start: number;
  end: number;
  onRange: (start: number, end: number) => void;
  /** Split points or bleep starts, in seconds. */
  markers?: number[];
  onMarkers?: (markers: number[]) => void;
  /** Draw the two end handles. The speed and split dialogs only scrub. */
  range?: boolean;
  /** The playhead moved, so "Add frame"/"Add split" can read it. */
  onPlayhead?: (seconds: number) => void;
  /** Playback multiplier: the speed dialog's factor. 1 plays in real time. */
  rate?: number;
}

/** One frame at 25 fps: the smallest trim nudge and the split dedupe radius. */
const FRAME = 1 / 25;
/** Playback stills are the file itself sampled; 10 fps reads as motion. */
const PLAYBACK_MAX = 240;
/** Step used until the playback frames land, or when the backend returns none. */
const FALLBACK_STEP = 0.1;
/** Playback tick, and the seconds travelled per tick at rate 1. */
const TICK_MS = 100;
const TICK_SECONDS = 0.1;

const clamp = (value: number, low: number, high: number): number =>
  Math.min(high, Math.max(low, value));

/** Transport icons: the bar is the "snap to this edge" half of a skip glyph. */
function SkipBackIcon() {
  return (
    <svg class="timeline-icon" viewBox="0 0 16 16" aria-hidden="true">
      <rect x="2.5" y="3" width="2" height="10" rx="1" />
      <path d="M13.5 3.2v9.6L5.8 8l7.7-4.8Z" />
    </svg>
  );
}

function PlayIcon() {
  return (
    <svg class="timeline-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path d="M4.2 2.6v10.8L13.6 8 4.2 2.6Z" />
    </svg>
  );
}

function PauseIcon() {
  return (
    <svg class="timeline-icon" viewBox="0 0 16 16" aria-hidden="true">
      <rect x="3.5" y="2.6" width="3" height="10.8" rx="1" />
      <rect x="9.5" y="2.6" width="3" height="10.8" rx="1" />
    </svg>
  );
}

function SkipForwardIcon() {
  return (
    <svg class="timeline-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path d="M2.5 3.2v9.6L10.2 8 2.5 3.2Z" />
      <rect x="11.5" y="3" width="2" height="10" rx="1" />
    </svg>
  );
}

/**
 * The video timeline: one large still for the playhead, the track underneath, the
 * range handles and the transport.
 *
 * Everything here is DOM state; dragging a handle never calls the engine. The job
 * runs on Apply, so a drag costs one style write — re-rendering a tree per pointer
 * event is the difference between smooth and laggy.
 */
export default function MediaTimeline(props: MediaTimelineProps) {
  const [frames, setFrames] = createSignal<string[]>([]);
  const [frameError, setFrameError] = createSignal('');
  const [playhead, setPlayhead] = createSignal(0);
  const [playing, setPlaying] = createSignal(false);
  /** No media element here: WebKitGTK's media stack crashed the app on Play. The
      still is the preview, and playing walks the playhead across the filmstrip. */
  let timer: number | undefined;
  /** Keyed by path: switching files elsewhere in the dialog reloads only the still. */
  let filmstripRequest = 0;

  createEffect(() => {
    const path = props.path;
    const seconds = props.duration;
    const request = ++filmstripRequest;
    if (!path || seconds <= 0) {
      setFrames([]);
      setFrameError('');
      return;
    }
    setFrames([]);
    setFrameError('');
    // Short clips get 10 fps; a long one is sampled over the same 240-frame budget,
    // so playback covers the clip instead of the first half minute.
    const fps = Math.max(1, Math.min(10, Math.floor(PLAYBACK_MAX / seconds)));
    void ipc
      .videoFrames(path, fps, PLAYBACK_MAX)
      .then((paths) => {
        if (request !== filmstripRequest) return;
        setFrames(paths.map((p) => ipc.assetUrl(p)));
      })
      .catch((error) => {
        if (request !== filmstripRequest) return;
        setFrameError(error instanceof Error ? error.message : 'No playback frames');
      });
  });

  const hasRange = (): boolean => props.range !== false;

  /** The playhead may never leave the trim span; other dialogs use the whole clip. */
  const low = (): number => (hasRange() ? clamp(props.start, 0, Math.max(0, props.duration)) : 0);
  const high = (): number =>
    hasRange()
      ? clamp(props.end, low(), Math.max(low(), props.duration))
      : Math.max(0, props.duration);

  /** The cursor is independent of the trim handles: dragging a handle must not move
      it. Playback, stepping and the playhead slider clamp into the trim span. */
  const displayPlayhead = (): number => clamp(playhead(), 0, Math.max(0, props.duration));

  const percent = (seconds: number): number =>
    props.duration > 0 ? clamp((seconds / props.duration) * 100, 0, 100) : 0;

  const rate = (): number => {
    const value = Number(props.rate ?? 1);
    return Number.isFinite(value) && value > 0 ? value : 1;
  };

  /** One filmstrip frame; the whole clip when the stills are not there yet. */
  const frameStep = (): number =>
    frames().length > 0 && props.duration > 0 ? props.duration / frames().length : FALLBACK_STEP;

  /** Which filmstrip still covers a moment; the end clamps to the last one. */
  const indexAt = (seconds: number): number => {
    const count = frames().length;
    if (count === 0 || props.duration <= 0) return 0;
    return clamp(Math.floor((seconds / props.duration) * count), 0, count - 1);
  };

  /** The still for that index; the memo keeps `src` from being rewritten per tick. */
  const currentFrame = createMemo((): string => {
    const list = frames();
    return list.length > 0 ? list[indexAt(displayPlayhead())] : '';
  });

  function seek(seconds: number): void {
    const next = clamp(seconds, low(), high());
    setPlayhead(next);
    props.onPlayhead?.(next);
  }

  function step(direction: number): void {
    const from = displayPlayhead();
    let next = clamp(from + direction * frameStep(), low(), high());
    // At the clip end the clamp lands on the last frame's own start; one more
    // frame back is what makes the visible still change.
    if (direction < 0 && indexAt(next) === indexAt(from)) {
      next = clamp(next - frameStep(), low(), high());
    }
    seek(next);
  }

  function stop(): void {
    window.clearInterval(timer);
    timer = undefined;
    setPlaying(false);
  }

  /** Playing walks the playhead over the filmstrip; it never decodes the file. */
  function togglePlay(): void {
    if (playing()) {
      stop();
      return;
    }
    if (props.duration <= 0) return;
    // Play always starts inside the trim span; at the end it rewinds to the start.
    if (playhead() < low() || playhead() >= high()) seek(low());
    setPlaying(true);
    timer = window.setInterval(() => {
      const next = displayPlayhead() + TICK_SECONDS * rate();
      if (next >= high()) {
        seek(high());
        stop();
        return;
      }
      seek(next);
    }, TICK_MS);
  }

  /** A handle drag moves the selection only; it must not drag the playhead. */
  function setStart(value: number): void {
    const next = clamp(value, 0, Math.max(0, props.end - FRAME));
    props.onRange(next, props.end);
  }

  function setEnd(value: number): void {
    const next = clamp(value, Math.min(props.duration, props.start + FRAME), props.duration);
    props.onRange(props.start, next);
  }

  function addSplit(): void {
    const markers = props.markers ?? [];
    const at = displayPlayhead();
    if (at <= 0 || at >= props.duration) return;
    if (markers.some((marker) => Math.abs(marker - at) < FRAME)) return;
    props.onMarkers?.([...markers, at].sort((a, b) => a - b));
  }

  function dropMarker(index: number): void {
    const markers = props.markers ?? [];
    props.onMarkers?.(markers.filter((_, position) => position !== index));
  }

  let markerDrag: { index: number; pointerId: number; startX: number } | null = null;

  /** Dragging a marker line moves the split; the × inside it deletes. */
  function markerDown(event: PointerEvent, index: number): void {
    const target = event.currentTarget as HTMLElement;
    markerDrag = { index, pointerId: event.pointerId, startX: event.clientX };
    target.setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function markerMove(event: PointerEvent): void {
    const drag = markerDrag;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (Math.abs(event.clientX - drag.startX) < 3) return;
    const track = (event.currentTarget as HTMLElement).parentElement;
    const rect = track?.getBoundingClientRect();
    if (!rect || rect.width === 0) return;
    const fraction = clamp((event.clientX - rect.left) / rect.width, 0, 1);
    const next = fraction * props.duration;
    const markers = (props.markers ?? []).map((marker, position) =>
      position === drag.index ? next : marker,
    );
    props.onMarkers?.(markers);
  }

  function markerUp(event: PointerEvent): void {
    const target = event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) {
      target.releasePointerCapture(event.pointerId);
    }
    markerDrag = null;
  }

  onCleanup(() => window.clearInterval(timer));

  return (
    <div class="media-timeline">
      <div class="timeline-stage">
        <div class="timeline-preview">
          <Show
            when={frames().length > 0}
            fallback={<div class="timeline-blank">{frameError() || 'Loading frames…'}</div>}
          >
            <img
              class="timeline-preview-frame"
              src={currentFrame()}
              alt=""
              draggable={false}
            />
          </Show>
        </div>

        <div class="timeline-track">
          <Show when={hasRange()}>
            <span
              class="timeline-selection"
              style={{
                left: `${percent(low())}%`,
                width: `${Math.max(0, percent(high()) - percent(low()))}%`,
              }}
            />
          </Show>
          <For each={props.markers ?? []}>
            {(marker, index) => (
              <span
                class="timeline-marker"
                style={{ left: `${percent(marker)}%` }}
                title={formatTimecode(marker)}
                onPointerDown={(event) => markerDown(event, index())}
                onPointerMove={markerMove}
                onPointerUp={markerUp}
                onPointerCancel={markerUp}
              >
                <span class="timeline-marker-line" aria-hidden="true" />
                <button
                  type="button"
                  class="timeline-marker-drop"
                  aria-label={`Delete split ${index() + 1}`}
                  onClick={() => dropMarker(index())}
                >
                  ×
                </button>
              </span>
            )}
          </For>
          <span class="timeline-playhead" style={{ left: `${percent(displayPlayhead())}%` }} />
        </div>
      </div>

      <div class="timeline-handles">
        <Show when={hasRange()}>
          <label class="timeline-handle">
            <span class="timeline-handle-label">Trim start</span>
            <input
              type="range"
              aria-label="Trim start"
              min={0}
              max={Math.max(0, props.duration)}
              step={FRAME}
              value={props.start}
              onInput={(event) => setStart(event.currentTarget.valueAsNumber)}
            />
          </label>
          <label class="timeline-handle">
            <span class="timeline-handle-label">Trim end</span>
            <input
              type="range"
              aria-label="Trim end"
              min={0}
              max={Math.max(0, props.duration)}
              step={FRAME}
              value={props.end}
              onInput={(event) => setEnd(event.currentTarget.valueAsNumber)}
            />
          </label>
        </Show>
      </div>

      <div class="timeline-handles">
        <label class="timeline-handle">
          <span class="timeline-handle-label">Playhead</span>
          <input
            type="range"
            aria-label="Playhead"
            min={0}
            max={Math.max(0, props.duration)}
            step={0.01}
            value={displayPlayhead()}
            onInput={(event) => seek(event.currentTarget.valueAsNumber)}
          />
        </label>
      </div>

      <div class="timeline-transport">
        <button
          type="button"
          class="timeline-button is-step"
          aria-label="Step back one frame"
          title="Step back one frame"
          onClick={() => step(-1)}
        >
          <SkipBackIcon />
        </button>
        <button
          type="button"
          class="timeline-button is-play"
          aria-label={playing() ? 'Pause' : 'Play'}
          title={playing() ? 'Pause' : 'Play'}
          disabled={props.duration <= 0}
          onClick={togglePlay}
        >
          <Show when={playing()} fallback={<PlayIcon />}>
            <PauseIcon />
          </Show>
        </button>
        <button
          type="button"
          class="timeline-button is-step"
          aria-label="Step forward one frame"
          title="Step forward one frame"
          onClick={() => step(1)}
        >
          <SkipForwardIcon />
        </button>
        <Show when={props.onMarkers}>
          <button
            type="button"
            class="timeline-button"
            aria-label="Add split at playhead"
            title="Add split at playhead"
            onClick={addSplit}
          >
            Add split
          </button>
        </Show>
        <span class="timeline-readout">
          <span class="timeline-position">
            {formatTimecode(displayPlayhead())}
            <span class="timeline-total"> / {formatTimecode(props.duration)}</span>
          </span>
          <Show when={hasRange()}>
            <span class="timeline-divider" aria-hidden="true" />
            <span class="timeline-range">
              <span class="timeline-range-label">trim</span>
              {formatTimecode(low())} – {formatTimecode(high())}
            </span>
          </Show>
        </span>
      </div>
    </div>
  );
}
