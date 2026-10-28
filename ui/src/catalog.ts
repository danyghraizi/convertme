import type { Segment } from './types';

export interface CategoryDef {
  id: string;
  label: string;
}

/** Fixed order and labels; the backend compares case-insensitively. */
export const CATEGORIES: CategoryDef[] = [
  { id: 'images', label: 'Images' },
  { id: 'video', label: 'Video' },
  { id: 'audio', label: 'Audio' },
  { id: 'pdf', label: 'PDF' },
  { id: 'documents', label: 'Documents' },
  { id: 'slides', label: 'Slides' },
  { id: 'spreadsheets', label: 'Spreadsheets' },
  { id: 'epub', label: 'EPUB' },
  { id: 'subtitles', label: 'Subtitles' },
  { id: 'archives', label: 'Archives' },
];

export function categoryLabel(id: string | null): string {
  if (!id) return 'No files';
  const match = CATEGORIES.find((category) => category.id === id.toLowerCase());
  return match ? match.label : id;
}

const seg = (id: string, label: string, blurb: string): Segment => ({ id, label, blurb });

/**
 * Tool lists per category. The wheel is the source of truth at runtime; this
 * table is the fallback the main window uses when `wheels(category, null)`
 * comes back empty, so the grid is never blank.
 */
export const FALLBACK_TOOLS: Record<string, Segment[]> = {
  images: [
    seg('compress', 'Compress to size', 'Shrink a photo to a target file size'),
    seg('crop', 'Crop', 'Cut the frame down to a chosen area'),
    seg('annotate', 'Annotate', 'Draw arrows, shapes and text'),
    seg('collage', 'Collage', 'Arrange several photos into one image'),
    seg('redact', 'Redact', 'Cover faces or text with solid blocks'),
    seg('metadata', 'Edit metadata', 'Read and write image tags'),
  ],
  video: [
    seg('compress', 'Compress to size', 'Two-pass encode to a target size'),
    seg('trim', 'Trim', 'Keep one section of the clip'),
    seg('crop', 'Crop', 'Change the frame of the video'),
    seg('speed', 'Change speed', 'Speed up or slow down, pitch kept'),
    seg('mute', 'Mute', 'Drop the audio track'),
    seg('snapshot', 'Save frames', 'Export still frames as images'),
    seg('join', 'Join clips', 'Concatenate several files in order'),
    seg('split', 'Split', 'Cut the clip into sections'),
    seg('redact', 'Redact', 'Blur a region over a time range'),
  ],
  audio: [
    seg('compress', 'Compress to size', 'Re-encode to a smaller file'),
    seg('normalize', 'Normalize', 'Even out loudness'),
    seg('trim', 'Trim', 'Keep one section of the track'),
    seg('channels', 'Channels', 'Mono or stereo output'),
    seg('bleep', 'Bleep', 'Silence a range and add a tone'),
    seg('visualize', 'Visualize', 'Render a waveform video'),
    seg('metadata', 'Edit metadata', 'Read and write audio tags'),
  ],
  pdf: [
    seg('compress', 'Compress', 'Re-distill pages at a smaller setting'),
    seg('merge', 'Merge', 'Join PDFs into one document'),
    seg('split', 'Split', 'One file per page'),
    seg('rotate', 'Rotate', 'Turn pages by 90 degree steps'),
    seg('organize', 'Organize pages', 'Reorder, rotate or drop pages'),
    seg('decrypt', 'Decrypt', 'Save without the password'),
  ],
  documents: [
    seg('compress', 'Compress', 'Repack a document with fewer bytes'),
    seg('merge', 'Merge', 'Append documents into one'),
    seg('metadata', 'Edit metadata', 'Change title, author and tags'),
  ],
  slides: [
    seg('compress', 'Compress', 'Downscale embedded images'),
    seg('extract', 'Extract images', 'Save every slide image'),
    seg('metadata', 'Edit metadata', 'Change title, author and tags'),
  ],
  spreadsheets: [
    seg('compress', 'Compress', 'Repack with a leaner archive'),
    seg('merge', 'Merge sheets', 'Combine workbooks into one'),
    seg('metadata', 'Edit metadata', 'Change title, author and tags'),
  ],
  epub: [
    seg('compress', 'Compress', 'Shrink embedded assets'),
    seg('metadata', 'Edit metadata', 'Change title, author and language'),
    seg('split', 'Split chapters', 'One file per chapter'),
  ],
  subtitles: [
    seg('shift', 'Shift timing', 'Move every cue by a fixed offset'),
    seg('merge', 'Merge', 'Concatenate subtitle files in order'),
    seg('strip', 'Strip formatting', 'Remove styling tags from cues'),
  ],
  archives: [
    seg('extract', 'Extract', 'Unpack into a folder beside the file'),
    seg('repack', 'Repack', 'Rewrite the archive in another format'),
    seg('list', 'List contents', 'Write an index of the archive'),
  ],
};

/** Target formats offered per category; ids match the registry's format ids. */
export const FALLBACK_TARGETS: Record<string, Segment[]> = {
  images: ['PNG', 'JPEG', 'WEBP', 'AVIF', 'TIFF', 'BMP', 'GIF', 'HEIC', 'ICO', 'PDF', 'JXL', 'TGA'].map(
    (label) => seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  video: ['MP4', 'MKV', 'WEBM', 'MOV', 'AVI', 'GIF', 'MP3', 'WAV', 'M4A'].map((label) =>
    seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  audio: ['MP3', 'M4A', 'AAC', 'FLAC', 'WAV', 'OGG', 'OPUS', 'AIFF', 'WMA'].map((label) =>
    seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  pdf: ['PNG', 'JPEG', 'TIFF', 'TXT', 'MD', 'DOCX', 'PPTX', 'XLSX', 'EPUB'].map((label) =>
    seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  documents: ['PDF', 'DOCX', 'ODT', 'MD', 'TXT', 'HTML', 'EPUB'].map((label) =>
    seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  slides: ['PDF', 'PNG', 'JPEG', 'ODP'].map((label) =>
    seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  spreadsheets: ['PDF', 'XLSX', 'ODS', 'CSV', 'TSV'].map((label) =>
    seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  epub: ['EPUB', 'DOCX', 'ODT', 'MD', 'TXT', 'HTML'].map((label) =>
    seg(label.toLowerCase(), label, `Convert to ${label}`),
  ),
  subtitles: ['SRT', 'VTT', 'TXT'].map((label) => seg(label.toLowerCase(), label, `Convert to ${label}`)),
  archives: ['ZIP', 'TAR', '7Z', 'GZ'].map((label) => seg(label.toLowerCase(), label, `Convert to ${label}`)),
};

/**
 * File actions. Ids are mapped to IPC calls by `performAction` in `actions.ts`;
 * copy and move are the only ones that touch the source location and only ever
 * run when picked directly.
 */
export const ACTIONS: Segment[] = [
  { id: 'share', label: 'Share', blurb: 'Hand the file to a nearby app through the system share sheet.' },
  { id: 'preview', label: 'Float', blurb: 'Open an always-on-top preview window.' },
  { id: 'copy', label: 'Copy', blurb: 'Copy the selection to another folder.' },
  {
    id: 'move',
    label: 'Move',
    blurb: 'Move the selection to another folder. This is the only action that relocates a file.',
  },
  { id: 'copy-path', label: 'Copy path', blurb: 'Put the full path on the clipboard.' },
  { id: 'terminal', label: 'Terminal', blurb: "Open a terminal in the file's folder." },
];
