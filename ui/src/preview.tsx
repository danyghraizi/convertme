import { Match, Switch, createSignal, onMount } from 'solid-js';
import { assetUrl } from './ipc';
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

/** Always-on-top preview window opened by the `preview` action. */
export default function PreviewWindow() {
  const params = new URLSearchParams(window.location.search);
  const path = params.get('path') ?? '';
  const kind = mediaKind(path);
  const [failed, setFailed] = createSignal(false);

  onMount(() => {
    document.documentElement.dataset.window = 'preview';
  });

  return (
    <div class="preview-window">
      <Switch>
        <Match when={path.length > 0 && !failed() && kind === 'image'}>
          <img
            class="preview-media"
            src={assetUrl(path)}
            alt={baseName(path)}
            onError={() => setFailed(true)}
          />
        </Match>
        <Match when={path.length > 0 && !failed() && kind === 'video'}>
          <video
            class="preview-media"
            src={assetUrl(path)}
            controls
            autoplay
            muted
            loop
            onError={() => setFailed(true)}
          />
        </Match>
        <Match when={path.length > 0 && !failed() && kind === 'audio'}>
          <div class="preview-audio">
            <p class="preview-name">{baseName(path)}</p>
            <audio src={assetUrl(path)} controls autoplay onError={() => setFailed(true)} />
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
