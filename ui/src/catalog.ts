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

const seg = (id: string, label: string, blurb: string): Segment => ({
  id,
  label,
  blurb,
  title: label,
  action: 'Run',
});

/**
 * The dialog copy for a tool: sheet title, primary action, the benefit lines and an
 * eyebrow naming the scope. These mirror `crates/convertme-core/src/formats.rs`, which
 * is the source of truth at runtime; this table is what the main window falls back to
 * when `wheels(category, null)` comes back empty. The dialog itself renders the title
 * and the action; the benefits are catalog copy, not a panel beside the sheet.
 */
const ref = (
  id: string,
  label: string,
  blurb: string,
  title: string,
  action: string,
  benefits: string[],
  eyebrow?: string,
): Segment => ({ id, label, blurb, title, action, benefits, eyebrow });

const ANY_FILE = 'Any file';

/**
 * Tool lists per category. An empty backend wheel is the only case that reaches
 * this table, so a missing entry here shows up as a category with no cards; the
 * ids and the copy are checked against the Rust table by the UI harness.
 */
export const FALLBACK_TOOLS: Record<string, Segment[]> = {
  images: [
    ref(
      'compress',
      'Compress to size',
      'Hit a target size or a percentage; the preview shows the result before you commit.',
      'Compress',
      'Compress',
      ['Target a file size or percentage reduction', 'Resize images while compressing'],
      ANY_FILE,
    ),
    ref(
      'crop',
      'Crop',
      'Drag the crop box or pick an aspect ratio.',
      'Crop images',
      'Apply',
      ['Enter exact pixel dimensions', 'Use preset or custom aspect ratios'],
    ),
    ref(
      'annotate',
      'Annotate',
      'Arrows, shapes, text and numbered callouts burned into a copy.',
      'Annotate photos',
      'Save Annotated Copy',
      ['Add numbered labels, arrows, and text', 'Draw or highlight directly on the photo'],
    ),
    ref(
      'collage',
      'Collage',
      'Arrange several images into one composition.',
      'Create a collage',
      'Create Collage',
      ['Grid, row, column, and featured layouts', 'Set final dimensions, spacing, corners, and background'],
    ),
    ref(
      'redact',
      'Redact',
      'Detect faces and text, then fill the boxes you keep.',
      'Redact photos',
      'Save Redacted Copy',
      ['Detect faces and text locally', 'Layer several movable redaction areas'],
    ),
    ref(
      'metadata',
      'Edit metadata',
      'Read, edit or strip camera and location tags across a selection.',
      'Edit metadata',
      'Save',
      ['Search fields grouped by file, track, or chapter', 'Remove hidden camera and processing data'],
      ANY_FILE,
    ),
    ref(
      'edit-photo',
      'Edit photo',
      'Adjust exposure, colour and detail without leaving the app.',
      'Edit photos',
      'Save Edited Copy',
      ['Dehaze, clarity, grain, and noise reduction', 'Fine controls for light, colour, and detail'],
    ),
    ref(
      'add-background',
      'Add background',
      'Place the photo on a colour, a gradient or another image, with padding and rounded corners.',
      'Add a background',
      'Save with Background',
      ['Use another image as the background', 'Add padding, rounded corners, and shadow'],
    ),
  ],
  video: [
    ref(
      'compress',
      'Compress to size',
      'Hit a target size with a two-pass encode that keeps the audio intact.',
      'Compress',
      'Compress',
      ['Shrink to a size or bitrate', 'Never grows the file'],
      ANY_FILE,
    ),
    ref(
      'trim',
      'Trim',
      'Drag the start and end handles on the filmstrip and step through frames.',
      'Trim video',
      'Apply',
      ['Step through the video frame by frame', 'Enter precise start and end timecodes'],
    ),
    ref(
      'crop',
      'Crop',
      'Drag the frame or pick an aspect ratio; the audio is untouched.',
      'Crop video',
      'Apply',
      ['Enter exact output dimensions', 'Keep the original audio'],
    ),
    ref(
      'speed',
      'Change speed',
      'Speed up or slow down the picture and keep the audio at its original pitch.',
      'Change video speed',
      'Apply',
      ['Preserve audio pitch', 'Works with silent videos'],
    ),
    seg('mute', 'Mute', 'Drop the audio track'),
    ref(
      'snapshot',
      'Save frames',
      'Export still frames as PNG at chosen timestamps or a fixed interval.',
      'Save video frames',
      'Save {n} Frame(s)',
      ['Choose frames one step at a time', 'Export full-resolution PNGs'],
    ),
    ref(
      'join',
      'Join clips',
      'Arrange clips in playback order and save one file.',
      'Join videos',
      'Join Videos',
      ['Mix MP4, MOV, and MKV clips', 'Use the first clip as the output canvas'],
    ),
    ref(
      'split',
      'Split',
      'Mark sections on the timeline and save each one as its own clip in a folder.',
      'Split video',
      'Split Video',
      ['Split at custom points or into equal parts', 'Place every output clip in one folder'],
    ),
    ref(
      'redact',
      'Redact',
      'Blur a region for a time range and re-encode a clean copy.',
      'Redact video',
      'Save Redacted Copy',
      ['Give each redaction its own time range', 'Use solid colors or blur'],
    ),
  ],
  audio: [
    ref(
      'compress',
      'Compress to size',
      'Shrink a recording to a target size or bitrate.',
      'Compress',
      'Compress',
      ['Shrink to a size or bitrate', 'Never grows the file'],
      ANY_FILE,
    ),
    ref(
      'normalize',
      'Normalize',
      'Even out loudness to a broadcast target.',
      'Normalize volume',
      'Normalize Audio',
      ['Control loudness range and true peak', 'Compare input and output levels'],
    ),
    ref(
      'trim',
      'Trim',
      'Cut with waveform handles and preview the edges.',
      'Trim audio',
      'Apply',
      ['Remove silence only from the beginning and end', 'Enter exact start and end times'],
    ),
    ref(
      'channels',
      'Channels',
      'Convert to mono or stereo.',
      'Convert audio channels',
      'Apply',
      ['Set left and right volume independently', 'Preview either channel on its own'],
    ),
    ref(
      'bleep',
      'Bleep',
      'Mark ranges on the waveform and replace them with a tone.',
      'Bleep audio',
      'Save Bleeped Copy',
      ['Move a whole bleep range or fine-tune its edges', 'Compare original and bleeped playback'],
    ),
    ref(
      'visualize',
      'Visualize',
      'Turn a recording into a waveform video on a background you pick.',
      'Create an audio visualizer',
      'Create Visualization',
      ['Use an animated waveform or a still image', 'Choose landscape, portrait, or square video'],
    ),
    ref(
      'metadata',
      'Edit metadata',
      'Edit title, artist, album and cover art.',
      'Edit metadata',
      'Save',
      ['Search fields grouped by file, track, or chapter', 'Remove hidden camera and processing data'],
      ANY_FILE,
    ),
  ],
  pdf: [
    ref(
      'compress',
      'Compress',
      'Downsample to a preset quality tier and keep the pages readable.',
      'Compress',
      'Compress',
      ['Choose a quality preset', 'Never grows the file'],
      ANY_FILE,
    ),
    ref(
      'merge',
      'Merge',
      'Combine the selected files into one document in selection order.',
      'Merge PDFs',
      'Merge PDFs',
      ['Start with the selection order', 'Reorder documents before merging'],
    ),
    seg('split', 'Split', 'One file per page'),
    seg('rotate', 'Rotate', 'Turn pages by 90 degree steps'),
    ref(
      'organize',
      'Organize pages',
      'Reorder, rotate, duplicate or delete pages, then save a new file.',
      'Organize PDF pages',
      'Save Organized PDF',
      ['Rotate, duplicate, or remove individual pages'],
    ),
    ref(
      'metadata',
      'Edit metadata',
      'Edit the document information dictionary.',
      'Edit metadata',
      'Save',
      ['Search fields grouped by file, track, or chapter', 'Remove hidden camera and processing data'],
      ANY_FILE,
    ),
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
