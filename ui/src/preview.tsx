import { Match, Show, Switch, createEffect, createSignal, onMount } from 'solid-js';
import * as ipc from './ipc';
import MediaTimeline from './components/media-timeline';
import { baseName } from './format';

type MediaKind = 'image' | 'video' | 'audio' | 'none';

const MEDIA: Record<string, MediaKind> = {
  png: 'image',
  jpg: 'image',
  jpeg: 'image',
  webp: 'image',
  avif: 'image',
  gif: 'image',
  bmp: 'image',
  tif: 'image',
  tiff: 'image',
  ico: 'image',
  svg: 'image',
  mp4: 'video',
  webm: 'video',
  ogv: 'video',
  mov: 'video',
  mkv: 'video',
  mp3: 'audio',
  wav: 'audio',
  ogg: 'audio',
  opus: 'audio',
  flac: 'audio',
  m4a: 'audio',
  aac: 'audio',
};

function mediaKind(path: string): MediaKind {
  const name = baseName(path);
  const dot = name.lastIndexOf('.');
  if (dot < 0) return 'none';
  return MEDIA[name.slice(dot + 1).toLowerCase()] ?? 'none';
}

/**
 * Always-on-top preview window opened by the `preview` action.
 *
 * It uses the same rule as every dialog: the web view never decodes media itself.
 * Video is the timeline's frame sequence (no `<video>`), and an audio preview is
 * played through the system sink (`play_preview`), so neither path can wedge the
 * web process (LESSONS 61).
 */
export default function PreviewWindow() {
  const params = new URLSearchParams(window.location.search);
  const path = params.get('path') ?? '';
  const kind = mediaKind(path);
  const [failed, setFailed] = createSignal(false);
  const [duration, setDuration] = createSignal(0);
  const [playing, setPlaying] = createSignal(false);

  onMount(() => {
    document.documentElement.dataset.window = 'preview';
  });

  createEffect(() => {
    if (kind !== 'video' || !path) return;
    void ipc
      .mediaInfo(path)
      .then((info) => setDuration(info.duration))
      .catch(() => setFailed(true));
  });

  async function playAudio(): Promise<void> {
    if (!path || playing()) return;
    setPlaying(true);
    setFailed(false);
    try {
      await ipc.playPreview(path);
    } catch {
      setFailed(true);
    } finally {
      setPlaying(false);
    }
  }

  return (
    <div class="preview-window">
      <Switch>
        <Match when={path.length > 0 && !failed() && kind === 'image'}>
          <div class="preview-media">
            <img src={ipc.assetUrl(path)} alt={baseName(path)} onError={() => setFailed(true)} />
          </div>
        </Match>
        <Match when={path.length > 0 && !failed() && kind === 'video'}>
          <div class="preview-media">
            <Show
              when={duration() > 0}
              fallback={<p class="preview-empty">Reading the clip…</p>}
            >
              <MediaTimeline
                path={path}
                duration={duration()}
                start={0}
                end={duration()}
                range={false}
                onRange={() => undefined}
              />
            </Show>
          </div>
        </Match>
        <Match when={path.length > 0 && !failed() && kind === 'audio'}>
          <div class="preview-media">
            <div class="preview-audio">
              <p class="preview-name">{baseName(path)}</p>
              <button type="button" class="button" disabled={playing()} onClick={() => void playAudio()}>
                {playing() ? 'Playing…' : 'Play'}
              </button>
            </div>
          </div>
        </Match>
        <Match when={true}>
          <div class="preview-empty">
            <p class="preview-name">{path.length > 0 ? baseName(path) : 'No file to preview'}</p>
            <p class="preview-copy">
              {failed()
                ? 'The file could not be rendered here.'
                : 'There is no inline preview for this format.'}
            </p>
          </div>
        </Match>
      </Switch>
    </div>
  );
}
