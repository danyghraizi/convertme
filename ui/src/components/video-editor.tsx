import { For, Show, createEffect, createMemo, createSignal } from 'solid-js';
import * as ipc from '../ipc';
import { asBoxes, asNumber } from '../fields';
import { formatTimecode } from '../format';
import CropEditor from './crop-editor';
import MediaTimeline from './media-timeline';
import RangeChips from './range-chips';
import { BoxesEditor } from './field-editor';
import './video-editor.css';
import type { Box, FieldValue, MediaInfo } from '../types';

/** A redaction box with its own time range; both keys travel to the engine. */
type TimedBox = Box & { start?: number; end?: number };

interface VideoEditorProps {
  tool: string;
  path: string | null;
  values: Record<string, FieldValue>;
  setValue: (key: string, value: FieldValue) => void;
  /** Parameters the editor owns and `collectParams` cannot express. */
  onExtras: (extras: Record<string, unknown>) => void;
  /** How many items a counted action names. */
  onCount: (count: number) => void;
  /** False disables the primary action (no split points, no frames). */
  onReady: (ready: boolean) => void;
  /** Bumped by the footer's Reset; the local marks belong to the dialog too. */
  resetToken: number;
}

const PLAYABLE: Record<string, true> = {
  mp4: true,
  webm: true,
  ogv: true,
  mov: true,
  m4v: true,
};

const round = (value: number): number => Math.round(value * 1000) / 1000;

/** Speed ticks from the reference; the slider itself is a field row. */
const SPEED_TICKS = [0.25, 0.5, 1, 1.5, 2, 4];

const hasTimedRange = (box: Box): box is TimedBox =>
  typeof (box as TimedBox).start === 'number' && typeof (box as TimedBox).end === 'number';

/**
 * The video dialog's surface: the filmstrip/timeline, the crop canvas and the
 * redaction boxes. Everything here is DOM state — only the primary action runs the
 * engine, so dragging a handle or a marker never spawns a process.
 *
 * The editor owns the values that have no generic control: `start`/`end` for the
 * trim handles, the split markers (turned into `sections`) and the frame chips
 * (turned into `times`). They reach the engine through `collectParams` for the
 * fields and through `onExtras` for the injected arrays.
 */
export default function VideoEditor(props: VideoEditorProps) {
  const [info, setInfo] = createSignal<MediaInfo | null>(null);
  const [frame, setFrame] = createSignal<string | null>(null);
  const [markers, setMarkers] = createSignal<number[]>([]);
  const [times, setTimes] = createSignal<number[]>([]);
  const [playhead, setPlayhead] = createSignal(0);
  const [equalClips, setEqualClips] = createSignal(4);

  const duration = (): number => info()?.duration ?? 0;
  const playable = (): boolean => {
    const format = info()?.format ?? '';
    return PLAYABLE[format.toLowerCase()] === true;
  };
  const start = (): number => asNumber(props.values.start ?? 0);
  const end = (): number => asNumber(props.values.end ?? 0);
  const boxes = (): TimedBox[] => asBoxes(props.values.boxes ?? []) as TimedBox[];

  // The first filmstrip frame is the crop/redaction preview: ImageMagick decodes a
  // video by walking it, which is why a dialog never asks for a full decode.
  createEffect(() => {
    const path = props.path;
    if (!path) {
      setFrame(null);
      return;
    }
    void ipc
      .filmstrip(path, 1)
      .then((frames) => setFrame(frames[0] ?? null))
      .catch(() => setFrame(null));
  });

  // Reset clears the marks the field store cannot hold (split points, frame chips).
  createEffect(() => {
    if (props.resetToken < 0) return;
    setMarkers([]);
    setTimes([]);
    setPlayhead(0);
  });

  // New file: the dialog's marks belong to the file they were drawn on.
  createEffect(() => {
    const path = props.path;
    if (!path) return;
    setInfo(null);
    setMarkers([]);
    setTimes([]);
    setPlayhead(0);
    void ipc
      .mediaInfo(path)
      .then((facts) => setInfo(facts))
      .catch(() => setInfo(null));
  });

  // Defaults per tool; the per-tool effects below override the ones they own.
  createEffect(() => {
    props.onExtras({});
    props.onCount(0);
    props.onReady(true);
  });

  // The trim handles start at the whole clip. Only an unset or invalid span is
  // replaced, so reopening the dialog keeps the user's choice.
  createEffect(() => {
    if (props.tool !== 'trim') return;
    const media = info();
    if (!media || media.duration <= 0) return;
    if (end() <= start() || end() > media.duration + 0.01) {
      props.setValue('start', 0);
      props.setValue('end', round(media.duration));
    }
  });

  // Split: consecutive pairs from [0, ...points, duration].
  createEffect(() => {
    if (props.tool !== 'split') return;
    const points = markers();
    const media = info();
    props.onReady(points.length > 0);
    if (!media || points.length === 0) {
      props.onExtras({});
      return;
    }
    const bounds = [0, ...points, media.duration];
    const sections = bounds
      .slice(1)
      .map((endOfSection, index) => ({ start: bounds[index], end: endOfSection }))
      .filter((section) => section.end - section.start > 0.01);
    props.onExtras({ sections });
  });

  // Frames: the chips are the value.
  createEffect(() => {
    if (props.tool !== 'snapshot') return;
    const chosen = times();
    props.onCount(chosen.length);
    props.onReady(chosen.length > 0);
    props.onExtras(chosen.length > 0 ? { times: chosen } : {});
  });

  // Crop: a free crop with no rectangle has nothing to submit; an aspect does.
  createEffect(() => {
    if (props.tool !== 'crop') return;
    const hasBox = asBoxes(props.values.boxes ?? []).length > 0;
    const aspect = String(props.values.aspect ?? 'free');
    props.onReady(hasBox || aspect !== 'free');
  });

  // Redaction: a drawn box starts out applying to the whole clip.
  createEffect(() => {
    if (props.tool !== 'redact') return;
    const media = info();
    const current = asBoxes(props.values.boxes ?? []);
    const missing = current.some((box) => !hasTimedRange(box));
    if (media && missing) {
      props.setValue(
        'boxes',
        current.map((box) =>
          hasTimedRange(box) ? box : { ...box, start: 0, end: round(media.duration) },
        ) as FieldValue,
      );
    }
    props.onReady(current.length > 0);
  });

  function setRedactBoxes(next: FieldValue): void {
    props.setValue('boxes', next);
  }

  function setRange(nextStart: number, nextEnd: number): void {
    props.setValue('start', round(nextStart));
    props.setValue('end', round(nextEnd));
  }

  /** W/H in source pixels: the number typed is the size the file will have. */
  const cropPixels = createMemo(() => {
    const media = info();
    const box = asBoxes(props.values.boxes ?? [])[0];
    if (!media || !box) return { w: 0, h: 0 };
    return { w: Math.round(box.w * media.width), h: Math.round(box.h * media.height) };
  });

  /** The rectangle to resize: the drawn one, or a centred full frame to start from. */
  function cropBox(): { box: { x: number; y: number; w: number; h: number } } | null {
    const media = info();
    if (!media) return null;
    const drawn = asBoxes(props.values.boxes ?? [])[0];
    if (drawn) return { box: drawn };
    return { box: { x: 0, y: 0, w: 1, h: 1 } };
  }

  // Typing a size must work before anything is drawn: the reference lets you enter the
  // exact output dimensions first, and a control that needs a prior drag is dead.
  function setCropWidth(width: number): void {
    const media = info();
    const current = cropBox();
    if (!media || !current || media.width === 0 || width <= 0) return;
    const fraction = Math.min(1, Math.max(0.02, width / media.width));
    props.setValue('boxes', [
      { ...current.box, w: fraction, x: Math.min(current.box.x, 1 - fraction) },
    ] as FieldValue);
  }

  function setCropHeight(height: number): void {
    const media = info();
    const current = cropBox();
    if (!media || !current || media.height === 0 || height <= 0) return;
    const fraction = Math.min(1, Math.max(0.02, height / media.height));
    props.setValue('boxes', [
      { ...current.box, h: fraction, y: Math.min(current.box.y, 1 - fraction) },
    ] as FieldValue);
  }

  function setEqualSplits(): void {
    const count = Math.max(2, Math.min(20, Math.round(equalClips())));
    const media = info();
    if (!media || media.duration <= 0) return;
    const points: number[] = [];
    for (let index = 1; index < count; index += 1) {
      points.push(round((media.duration * index) / count));
    }
    setMarkers(points);
  }

  function updateBoxRange(index: number, key: 'start' | 'end', value: number): void {
    const next = boxes().map((box, position) =>
      position === index ? { ...box, [key]: value } : box,
    );
    props.setValue('boxes', next as FieldValue);
  }

  return (
    <div class="video-editor">
      <Show when={info()}>
        {(media) => (
          <Show when={props.tool === 'trim'}>
            <div class="field is-block">
              <span class="field-label">Trim</span>
              <MediaTimeline
                path={props.path}
                duration={media().duration}
                start={start()}
                end={end()}
                playable={playable()}
                onRange={setRange}
                onPlayhead={setPlayhead}
              />
            </div>
          </Show>
        )}
      </Show>

      <Show when={props.tool === 'crop'}>
        <div class="field is-block">
          <span class="field-label">Area to keep</span>
          <CropEditor
            previewPath={frame()}
            value={asBoxes(props.values.boxes ?? [])[0] ?? null}
            aspect={String(props.values.aspect ?? 'free')}
            onChange={(box) => props.setValue('boxes', [box] as FieldValue)}
          />
          <div class="crop-size-row">
            <label class="crop-size">
              <span>Width</span>
              <input
                type="number"
                aria-label="Width"
                min={1}
                value={cropPixels().w}
                onInput={(event) => setCropWidth(event.currentTarget.valueAsNumber)}
              />
              <span class="field-unit">px</span>
            </label>
            <label class="crop-size">
              <span>Height</span>
              <input
                type="number"
                aria-label="Height"
                min={1}
                value={cropPixels().h}
                onInput={(event) => setCropHeight(event.currentTarget.valueAsNumber)}
              />
              <span class="field-unit">px</span>
            </label>
            <button
              type="button"
              class="button button-quiet"
              onClick={() => props.setValue('boxes', [] as FieldValue)}
            >
              Reset crop
            </button>
          </div>
        </div>
        <Show when={info()}>
          {(media) => (
            <div class="field is-block">
              <span class="field-label">Step through</span>
              <MediaTimeline
                path={props.path}
                duration={media().duration}
                start={0}
                end={media().duration}
                range={false}
                playable={playable()}
                onRange={() => undefined}
              />
            </div>
          )}
        </Show>
      </Show>

      <Show when={props.tool === 'speed'}>
        <div class="field is-block">
          <span class="field-label">Preview a frame</span>
          <Show when={info()}>
            {(media) => (
              <MediaTimeline
                path={props.path}
                duration={media().duration}
                start={0}
                end={media().duration}
                range={false}
                playable={playable()}
                onRange={() => undefined}
              />
            )}
          </Show>
          <div class="speed-ticks" aria-hidden="true">
            <For each={SPEED_TICKS}>
              {(tick) => (
                <button
                  type="button"
                  class="speed-tick"
                  classList={{ 'is-active': Math.abs(asNumber(props.values.factor ?? 1) - tick) < 0.03 }}
                  onClick={() => props.setValue('factor', tick)}
                >
                  {tick}×
                </button>
              )}
            </For>
          </div>
        </div>
      </Show>

      <Show when={props.tool === 'split'}>
        <div class="field is-block">
          <span class="field-label">Split points</span>
          <Show when={info()}>
            {(media) => (
              <MediaTimeline
                path={props.path}
                duration={media().duration}
                start={0}
                end={media().duration}
                range={false}
                playable={playable()}
                markers={markers()}
                onMarkers={setMarkers}
                onRange={() => undefined}
                onPlayhead={setPlayhead}
              />
            )}
          </Show>
          <div class="split-equal">
            <label class="split-equal-label">
              <span>Equal clips</span>
              <input
                type="number"
                aria-label="Equal clips"
                min={2}
                max={20}
                value={equalClips()}
                onInput={(event) => setEqualClips(event.currentTarget.valueAsNumber)}
              />
            </label>
            <button type="button" class="button button-quiet" onClick={setEqualSplits}>
              Set
            </button>
          </div>
          <p class="field-help">
            {markers().length === 0
              ? 'Move the playhead and press Add split, or set equal clips.'
              : `${markers().length + 1} clips: ${[0, ...markers(), duration()]
                  .map((point) => formatTimecode(point))
                  .join(' – ')}`}
          </p>
          <span class="editor-playhead" aria-hidden="true">
            {formatTimecode(playhead())}
          </span>
        </div>
      </Show>

      <Show when={props.tool === 'snapshot'}>
        <div class="field is-block">
          <span class="field-label">Frames</span>
          <Show when={info()}>
            {(media) => (
              <MediaTimeline
                path={props.path}
                duration={media().duration}
                start={0}
                end={media().duration}
                range={false}
                playable={playable()}
                onRange={() => undefined}
                onPlayhead={setPlayhead}
              />
            )}
          </Show>
          <RangeChips
            times={times()}
            onChange={setTimes}
            playhead={playhead()}
            max={50}
            addLabel="Add frame"
            emptyHelp="Step to a moment and add it."
          />
        </div>
      </Show>

      <Show when={props.tool === 'redact'}>
        <div class="field is-block">
          <span class="field-label">Areas to redact</span>
          <BoxesEditor
            value={props.values.boxes ?? []}
            previewPath={frame()}
            onChange={setRedactBoxes}
          />
          <ol class="redaction-list">
            <For each={boxes()}>
              {(box, index) => (
                <li class="redaction-row">
                  <span class="redaction-label">Region {index() + 1}</span>
                  <label class="redaction-time">
                    <span>start</span>
                    <input
                      type="number"
                      aria-label={`Region ${index() + 1} start`}
                      min={0}
                      step={0.1}
                      value={box.start ?? 0}
                      onInput={(event) =>
                        updateBoxRange(index(), 'start', event.currentTarget.valueAsNumber)
                      }
                    />
                  </label>
                  <label class="redaction-time">
                    <span>end</span>
                    <input
                      type="number"
                      aria-label={`Region ${index() + 1} end`}
                      min={0}
                      step={0.1}
                      value={box.end ?? duration()}
                      onInput={(event) =>
                        updateBoxRange(index(), 'end', event.currentTarget.valueAsNumber)
                      }
                    />
                  </label>
                  <button
                    type="button"
                    class="button button-quiet"
                    aria-label={`Remove region ${index() + 1}`}
                    onClick={() =>
                      setRedactBoxes(
                        boxes().filter((_, position) => position !== index()) as FieldValue,
                      )
                    }
                  >
                    ×
                  </button>
                </li>
              )}
            </For>
          </ol>
        </div>
      </Show>
    </div>
  );
}
