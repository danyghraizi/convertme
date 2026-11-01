/**
 * The two transport glyphs, shared by the waveform transport and the audio preview
 * buttons. An icon says "play/pause" without the button changing its width or the
 * label losing its meaning.
 */
export function PlayGlyph() {
  return (
    <svg class="transport-glyph" viewBox="0 0 12 12" aria-hidden="true">
      <polygon points="2.5,1.5 10.5,6 2.5,10.5" />
    </svg>
  );
}

export function PauseGlyph() {
  return (
    <svg class="transport-glyph" viewBox="0 0 12 12" aria-hidden="true">
      <rect x="2.5" y="1.5" width="2.8" height="9" />
      <rect x="6.7" y="1.5" width="2.8" height="9" />
    </svg>
  );
}
