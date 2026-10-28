// Self-hosted, OFL families. Only the chosen pairing ships in production; the
// review build loads all of them so the options can be compared side by side.
import '@fontsource/ibm-plex-sans/400.css';
import '@fontsource/ibm-plex-sans/500.css';
import '@fontsource/ibm-plex-sans/600.css';
import '@fontsource/ibm-plex-mono/400.css';
import '@fontsource/ibm-plex-mono/500.css';
import '@fontsource/space-grotesk/500.css';
import '@fontsource/space-grotesk/600.css';
import '@fontsource/source-sans-3/400.css';
import '@fontsource/source-sans-3/600.css';
import { render } from 'solid-js/web';
import App from './app';
import WheelWindow from './wheel';
import PreviewWindow from './preview';
import { currentWindowLabel } from './ipc';
import './styles.css';

const root = document.getElementById('root');
if (!root) throw new Error('ConvertMe UI: #root is missing from index.html');

// Design-review switches: `?font=plex|grotesk|source|system&palette=paper|amber|category&density=compact`.
// They only set data attributes, so every component stays token-driven.
const query = new URLSearchParams(window.location.search);
for (const axis of ['font', 'palette', 'density', 'wheel'] as const) {
  const value = query.get(axis);
  if (value) document.documentElement.dataset[axis] = value;
}

const label = currentWindowLabel();
const view = new URLSearchParams(window.location.search).get('view');
document.documentElement.dataset.window = view === 'preview' ? 'preview' : label;

render(() => {
  if (view === 'preview') return <PreviewWindow />;
  if (label === 'wheel') return <WheelWindow />;
  return <App />;
}, root);
