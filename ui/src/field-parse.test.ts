import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFields } from './field-parse.ts';

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
