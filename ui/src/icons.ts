/**
 * Line glyphs for the wheel: one 24×24 path per icon, drawn with `stroke:
 * currentColor`, so an icon inherits the segment's ink and flips to white with the
 * label when the segment is selected.
 *
 * The set is deliberately small and shared: a format uses the glyph of its family
 * (image, page, sheet, slides, book, caption, box, play, note) and a tool uses the
 * glyph closest to what it does. Anything unmapped falls back to a plain tile.
 */
export type Glyph = keyof typeof GLYPHS;

export const GLYPHS = {
  image:
    'M4 5h16v14H4z M4 15l4.5-4.5L13 15l3-3 4 4 M8.5 9.5a1.5 1.5 0 1 0 0-.01',
  page: 'M7 3h7l4 4v14H7z M14 3v5h4',
  sheet: 'M4 4h16v16H4z M4 9h16 M9 4v16 M14 4v16',
  slides: 'M3 5h18v11H3z M8 20h8 M12 16v4',
  book: 'M5 4h6a3 3 0 0 1 3 3v13a2 2 0 0 0-2-2H5z M19 4h-3a3 3 0 0 0-3 3v13a2 2 0 0 1 2-2h4z',
  caption: 'M4 6h16v9H4z M8 19h8 M4 9h5',
  box: 'M4 7l8-3 8 3v10l-8 3-8-3z M4 7l8 3 8-3 M12 10v10',
  play: 'M4 5h16v14H4z M10 9l5 3-5 3z',
  note: 'M9 18V6l9-2v12 M9 18a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0',
  compress: 'M10 4v6H4 M14 20v-6h6 M4 14l5 5 M20 10l-5-5',
  crop: 'M6 3v13a2 2 0 0 0 2 2h13 M3 6h13a2 2 0 0 1 2 2v13',
  pen: 'M4 20l4-1 10-10a2.1 2.1 0 0 0-3-3L5 16z M14 6l4 4',
  tag: 'M4 10V4h6l9 9-6 6z M8 8h.01',
  eyeOff:
    'M3 3l18 18 M10.6 5.1A9.9 9.9 0 0 1 12 5c6 0 9 7 9 7a15 15 0 0 1-3.2 4 M6.2 6.4A15.6 15.6 0 0 0 3 12s3 7 9 7c1.7 0 3.2-.5 4.5-1.3',
  scissors:
    'M6 6a2 2 0 1 1 0-.01 M6 18a2 2 0 1 1 0-.01 M7.5 7.5L20 18 M7.5 16.5L20 6',
  gauge: 'M12 21a9 9 0 1 1 0-18 9 9 0 0 1 0 18z M12 12l4-4',
  speakerOff: 'M4 9h3l4-4v14l-4-4H4z M16 9l5 6 M21 9l-5 6',
  camera: 'M4 8h3l2-3h6l2 3h3v11H4z M12 17a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z',
  layers: 'M12 3l9 5-9 5-9-5z M3 13l9 5 9-5 M3 17l9 5 9-5',
  wave: 'M2 12h3l2-6 3 12 3-9 2 6 2-3h5',
  sliders: 'M5 7h14 M5 12h14 M5 17h14 M9 5v4 M15 10v4 M7 15v4',
  tile: 'M4 4h16v16H4z M4 10h16 M10 4v16',
} as const;

/** Which glyph a segment uses, by format id or tool id. */
const BY_ID: Record<string, Glyph> = {
  // formats
  PNG: 'image', JPG: 'image', WEBP: 'image', HEIC: 'image', TIFF: 'image', AVIF: 'image',
  BMP: 'image', SVG: 'image', GIF: 'image', ICO: 'image', TGA: 'image', JXL: 'image',
  PDF: 'page', DOCX: 'page', ODT: 'page', TXT: 'page', MD: 'page', HTML: 'page',
  XLSX: 'sheet', XLS: 'sheet', ODS: 'sheet', CSV: 'sheet', TSV: 'sheet',
  PPTX: 'slides', PPT: 'slides', EPUB: 'book',
  SRT: 'caption', VTT: 'caption',
  ZIP: 'box', TAR: 'box', GZIP: 'box', RAR: 'box',
  MP4: 'play', MOV: 'play', MKV: 'play', WEBM: 'play', AVI: 'play', WMV: 'play',
  MP3: 'note', M4A: 'note', WAV: 'note', FLAC: 'note', OGG: 'note', OPUS: 'note',
  AIFF: 'note', WMA: 'note',
  // tools
  compress: 'compress', 'add-background': 'tile', crop: 'crop', annotate: 'pen',
  'edit-photo': 'sliders', metadata: 'tag', redact: 'eyeOff', trim: 'scissors',
  split: 'scissors', speed: 'gauge', mute: 'speakerOff', snapshot: 'camera',
  join: 'layers', merge: 'layers', normalize: 'wave', channels: 'sliders',
  visualize: 'wave', bleep: 'speakerOff', organize: 'layers', extract: 'box',
  'save-pages': 'image', 'extract-text': 'page',
  // actions
  preview: 'image', copy: 'layers', move: 'layers', 'copy-path': 'tag',
  terminal: 'tile', share: 'box',
};

export function glyphFor(id: string): string {
  // Segment ids arrive canonical from the registry ("WEBP") but the mock and older
  // persisted settings may use lowercase; both resolve to the same glyph.
  return GLYPHS[BY_ID[id] ?? BY_ID[id.toUpperCase()] ?? BY_ID[id.toLowerCase()] ?? 'tile'];
}
