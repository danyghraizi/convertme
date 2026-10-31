import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
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
  /** Container the browser can play; everything else is filmstrip-only. */
  playable?: boolean;
}

/** One frame at 25 fps, the step the reference offers. */
const FRAME = 1 / 25;

const clamp = (value: number, low: number, high: number): number =>
  Math.min(high, Math.max(low, value));

/**
 * The video timeline: a filmstrip, a playhead, two end handles and a transport.
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
  const [failedPlayback, setFailedPlayback] = createSignal(false);
  let video: HTMLVideoElement | undefined;

  onMount(() => {
    const path = props.path;
    if (!path) return;
    void ipc
      .filmstrip(path, 12)
      .then((paths) => setFrames(paths.map((p) => ipc.assetUrl(p))))
      .catch((error) => setFrameError(error instanceof Error ? error.message : 'No filmstrip'));
  });

  const canPlay = (): boolean => Boolean(props.playable && props.path && !failedPlayback());
  const percent = (seconds: number): number =>
    props.duration > 0 ? clamp((seconds / props.duration) * 100, 0, 100) : 0;

  function seek(seconds: number): void {
    const next = clamp(seconds, 0, props.duration);
    setPlayhead(next);
    if (video) video.currentTime = next;
  }

  function step(direction: number): void {
    seek(playhead() + direction * FRAME);
  }

  function togglePlay(): void {
    if (!video || !canPlay()) return;
    if (playing()) {
      video.pause();
    } else {
      void video.play().catch(() => setFailedPlayback(true));
    }
  }

  function setStart(value: number): void {
    const next = clamp(value, 0, Math.max(0, props.end - FRAME));
    props.onRange(next, props.end);
    seek(next);
  }

  function setEnd(value: number): void {
    const next = clamp(value, Math.min(props.duration, props.start + FRAME), props.duration);
    props.onRange(props.start, next);
    seek(next);
  }

  function addSplit(): void {
    const markers = props.markers ?? [];
    const at = playhead();
    if (at <= 0 || at >= props.duration) return;
    if (markers.some((marker) => Math.abs(marker - at) < FRAME)) return;
    props.onMarkers?.([...markers, at].sort((a, b) => a - b));
  }

  function dropMarker(index: number): void {
    const markers = props.markers ?? [];
    props.onMarkers?.(markers.filter((_, position) => position !== index));
  }

  onCleanup(() => {
    video?.pause();
  });

  return (
    <div class="media-timeline">
      <div class="timeline-stage">
        <Show
          when={canPlay()}
          fallback={
            <div class="timeline-strip" aria-hidden="true">
              <Show
                when={frames().length > 0}
                fallback={<div class="timeline-blank">{frameError() || 'Loading frames…'}</div>}
              >
                <For each={frames()}>
                  {(frame) => <img class="timeline-frame" src={frame} alt="" draggable={false} />}
                </For>
              </Show>
            </div>
          }
        >
          <video
            ref={video}
            class="timeline-video"
            src={props.path ? ipc.assetUrl(props.path) : undefined}
            preload="metadata"
            onTimeUpdate={(event) => setPlayhead(event.currentTarget.currentTime)}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onEnded={() => setPlaying(false)}
            onError={() => setFailedPlayback(true)}
          />
        </Show>

        <div class="timeline-track">
          <Show when={props.range !== false}>
            <span
              class="timeline-selection"
              style={{
                left: `${percent(props.start)}%`,
                width: `${Math.max(0, percent(props.end) - percent(props.start))}%`,
              }}
            />
          </Show>
          <For each={props.markers ?? []}>
            {(marker, index) => (
              <button
                type="button"
                class="timeline-marker"
                style={{ left: `${percent(marker)}%` }}
                aria-label={`Delete split ${index() + 1}`}
                title={`${formatTimecode(marker)} · delete`}
                onClick={() => dropMarker(index())}
              />
            )}
          </For>
          <span class="timeline-playhead" style={{ left: `${percent(playhead())}%` }} />
        </div>
      </div>

      <div class="timeline-handles">
        <Show when={props.range !== false}>
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

      <div class="timeline-transport">
        <button
          type="button"
          class="timeline-button"
          aria-label="Step back"
          onClick={() => step(-1)}
        >
          ◀︎
        </button>
        <button
          type="button"
          class="timeline-button"
          aria-label={playing() ? 'Pause' : 'Play'}
          disabled={!canPlay()}
          onClick={togglePlay}
        >
          {playing() ? '❚❚' : '▶'}
        </button>
        <button
          type="button"
          class="timeline-button"
          aria-label="Step forward"
          onClick={() => step(1)}
        >
          ▶︎
        </button>
        <Show when={props.onMarkers}>
          <button type="button" class="timeline-button" aria-label="Add split at playhead" onClick={addSplit}>
            Add split
          </button>
        </Show>
        <span class="timeline-readout">
          {formatTimecode(playhead())}
          <span class="timeline-total"> / {formatTimecode(props.duration)}</span>
        </span>
        <Show when={props.range !== false}>
          <span class="timeline-range">
            {formatTimecode(props.start)} – {formatTimecode(props.end)}
          </span>
        </Show>
      </div>

      <Show when={props.playable && failedPlayback()}>
        <p class="field-help">
          This container cannot be played in the window; step through the frames instead.
        </p>
      </Show>
    </div>
  );
}
