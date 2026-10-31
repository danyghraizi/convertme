import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFields, readSegmentPayload } from './field-parse.ts';
import { collectParams } from './fields.ts';

/**
 * The dialog can only render what this reader keeps. Dropping `kind` turned the
 * gradient editor, the ratio buttons and the file picker into text boxes; dropping
 * `show_if` and `option_labels` showed every fill field at once with raw values.
 */
test('every control kind and its metadata survive the trip from Rust', () => {
  const parsed = readFields([
    {
      key: 'mode',
      label: 'Background',
      kind: 'select',
      default: 'colour',
      options: ['colour', 'gradient', 'image'],
      option_labels: ['A colour', 'A gradient', 'Another image'],
      control: true,
    },
    { key: 'gradient', label: 'Gradient', kind: 'gradient', default: 'linear|0|#000000@0,#ffffff@100' },
    { key: 'image', label: 'Background image', kind: 'file', default: '' },
    {
      key: 'aspect',
      label: 'Canvas ratio',
      kind: 'segmented',
      default: 'free',
      options: ['free', '1:1'],
      option_labels: ['Auto', '1:1'],
    },
    {
      key: 'colour',
      label: 'Colour',
      kind: 'color',
      default: '#101418',
      show_if: { key: 'mode', equals: 'colour' },
    },
    { key: 'padding', label: 'Padding', kind: 'range', default: 64, min: 0, max: 400, step: 2, unit: 'px' },
  ]);

  const by = (key: string) => parsed.find((field) => field.key === key)!;
  assert.equal(by('gradient').kind, 'gradient');
  assert.equal(by('image').kind, 'file');
  assert.equal(by('aspect').kind, 'segmented');
  assert.deepEqual(by('aspect').option_labels, ['Auto', '1:1']);
  assert.deepEqual(by('mode').option_labels, ['A colour', 'A gradient', 'Another image']);
  assert.equal(by('mode').control, true);
  assert.deepEqual(by('colour').show_if, { key: 'mode', equals: 'colour' });
  assert.equal(by('padding').kind, 'range');
  assert.equal(by('padding').unit, 'px');
  assert.equal(by('padding').default, 64);
});

test('an unknown kind still renders as text instead of vanishing', () => {
  const [field] = readFields([{ key: 'x', label: 'X', kind: 'hologram', default: 'v' }]);
  assert.equal(field.kind, 'text');
});

/**
 * The segment is the whole dialog's copy: heading, primary button, rail's check
 * lines and eyebrow. A reader that dropped them rendered a sheet with no title and
 * a rail with no words.
 */
test('a full segment payload keeps every key', () => {
  const segment = readSegmentPayload({
    id: 'organize',
    label: 'Organize pages',
    blurb: 'Reorder, rotate, duplicate or delete pages, then save a new file.',
    title: 'Organize PDF pages',
    action: 'Save Organized PDF',
    benefits: ['Rotate, duplicate, or remove individual pages'],
    eyebrow: 'Any file',
    disabled: false,
    hint: 'qpdf is not installed',
  });
  assert.equal(segment.id, 'organize');
  assert.equal(segment.label, 'Organize pages');
  assert.equal(segment.title, 'Organize PDF pages');
  assert.equal(segment.action, 'Save Organized PDF');
  assert.deepEqual(segment.benefits, ['Rotate, duplicate, or remove individual pages']);
  assert.equal(segment.eyebrow, 'Any file');
  assert.equal(segment.disabled, false);
  assert.equal(segment.hint, 'qpdf is not installed');
});

test('a convert target carries no dialog copy', () => {
  const segment = readSegmentPayload({ id: 'png', label: 'PNG', blurb: 'Convert to PNG' });
  assert.equal(segment.title, undefined);
  assert.equal(segment.action, undefined);
  assert.equal(segment.benefits, undefined);
  assert.equal(segment.eyebrow, undefined);
});

test('junk in the benefits list is dropped, not rendered', () => {
  const segment = readSegmentPayload({
    id: 'x',
    label: 'X',
    benefits: ['one', 2, null, 'two'],
  });
  assert.deepEqual(segment.benefits, ['one', 'two']);
});

/**
 * The four editors that are not a control: shapes, the page grid, the file order and
 * the metadata list. Each arrives as a structured value, and the value is what the
 * dialog submits — so the reader and the collector are tested as one contract.
 */
test('structured kinds survive the trip and flatten into the engine’s keys', () => {
  const parsed = readFields([
    {
      key: 'shapes',
      label: 'Annotations',
      kind: 'shapes',
      default: [
        { kind: 'rect', x: 0.1, y: 0.1, w: 0.2, h: 0.2, color: '#ff0000', size: 4 },
        { kind: 'text', x: 0.5, y: 0.5, w: 0.1, h: 0.05, text: 'Hi' },
        { kind: 'hologram', x: 0, y: 0, w: 1, h: 1 },
      ],
      hidden: true,
    },
    {
      key: 'pages',
      label: 'Pages',
      kind: 'pages',
      default: { order: [1, 2], rotate: [0, 90] },
    },
    { key: 'files', label: 'Order', kind: 'files', default: [2, 0, 1], hidden: true },
    {
      key: 'meta',
      label: 'Metadata',
      kind: 'metadata',
      default: { set: { Title: 'Alignment' }, remove: ['Author'], strip: false },
    },
  ]);

  const by = (key: string) => parsed.find((field) => field.key === key)!;
  assert.equal(by('shapes').kind, 'shapes');
  assert.deepEqual(by('shapes').default, [
    { kind: 'rect', x: 0.1, y: 0.1, w: 0.2, h: 0.2, color: '#ff0000', size: 4 },
    { kind: 'text', x: 0.5, y: 0.5, w: 0.1, h: 0.05, text: 'Hi' },
  ]);
  // Hidden travels with the field: the dialog draws it in its own editor.
  assert.equal(by('shapes').hidden, true);
  assert.deepEqual(by('pages').default, { order: [1, 2], rotate: [0, 90] });
  assert.deepEqual(by('files').default, [2, 0, 1]);
  assert.deepEqual(by('meta').default, {
    set: { Title: 'Alignment' },
    remove: ['Author'],
    strip: false,
  });

  // The page grid is one control over `order` and `rotate`; a zero rotation is not a
  // rotation, so it is not sent (position 1 stays out of the object).
  const pageParams = collectParams(parsed, {
    shapes: [],
    pages: { order: [2, 1], rotate: [90, 0] },
    files: [1, 0],
    meta: { set: {}, remove: [], strip: false },
  });
  assert.deepEqual(pageParams.order, [2, 1]);
  assert.deepEqual(pageParams.rotate, { '1': 90 });
  assert.deepEqual(pageParams.files, [1, 0]);
  // The metadata editor is one control over three keys, and an empty part is omitted.
  assert.deepEqual(pageParams.set, undefined);
  assert.deepEqual(pageParams.remove, undefined);
  assert.equal(pageParams.strip, false);
});

test('a metadata edit sends only the keys it changed', () => {
  const [field] = readFields([
    { key: 'meta', label: 'Metadata', kind: 'metadata', default: { set: {}, remove: [], strip: false } },
  ]);
  const params = collectParams([field], {
    meta: { set: { Title: 'Alignment' }, remove: ['Author'], strip: true },
  });
  assert.deepEqual(params.set, { Title: 'Alignment' });
  assert.deepEqual(params.remove, ['Author']);
  assert.equal(params.strip, true);
});
