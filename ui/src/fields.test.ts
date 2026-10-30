import assert from 'node:assert/strict';
import { test } from 'node:test';
import { collectParams } from './fields.ts';
import type { Field } from './types.ts';

/**
 * The per-file parameters are collected from the store, and the backend rejects any key
 * the tool does not know: the mode switch leaked as `mode` and the run failed with
 * "unknown parameter `mode`".
 */
const fields: Field[] = [
  {
    key: 'mode',
    label: 'Background',
    kind: 'select',
    default: 'colour',
    options: ['colour', 'gradient'],
    control: true,
  },
  {
    key: 'color',
    label: 'Colour',
    kind: 'color',
    default: '#101418',
    show_if: { key: 'mode', equals: 'colour' },
  },
  {
    key: 'gradient',
    label: 'Gradient',
    kind: 'gradient',
    default: 'linear|0|#000000@0,#ffffff@100',
    show_if: { key: 'mode', equals: 'gradient' },
  },
  { key: 'padding_pct', label: 'Padding', kind: 'range', default: 4, min: 0, max: 100, step: 1 },
];

test('a control is never sent as a parameter', () => {
  const params = collectParams(fields, { mode: 'gradient', padding_pct: 20 });
  assert.equal('mode' in params, false);
  assert.equal(params.padding_pct, 20);
});

test('a field hidden by show_if stays at home', () => {
  const colour = collectParams(fields, { mode: 'colour', color: '#ff0000' });
  assert.equal('gradient' in colour, false);
  const gradient = collectParams(fields, { mode: 'gradient', gradient: 'linear|0|#000000@0,#ffffff@100' });
  assert.equal('color' in gradient, false);
  assert.equal(typeof gradient.gradient, 'string');
});

test('a numeric select still becomes a number', () => {
  const numeric: Field[] = [
    { key: 'scale', label: 'Scale', kind: 'select', default: '2', options: ['1', '2', '100%'] },
  ];
  assert.deepEqual(collectParams(numeric, { scale: '2' }), { scale: 2 });
  assert.deepEqual(collectParams(numeric, { scale: '100%' }), { scale: '100%' });
});

/**
 * The image-compress target switch drives `target_bytes`; if it travelled, the backend
 * rejected the whole job with "unknown parameter `target`". The same field list is
 * asserted to carry `control` on the Rust side (`field_payload`).
 */
test('the compress target switch never becomes a job parameter', () => {
  const compress: Field[] = [
    { key: 'quality', label: 'Quality', kind: 'range', default: 80 },
    {
      key: 'scale',
      label: 'Image size',
      kind: 'select',
      default: '100',
      options: ['100', '75', '50', '25'],
    },
    { key: 'reduce', label: 'Reduce colours (lossy)', kind: 'checkbox', default: false, control: true },
    {
      key: 'colours',
      label: 'Colours',
      kind: 'select',
      default: '256',
      options: ['256', '128', '64', '32', '16'],
      show_if: { key: 'reduce', equals: 'true' },
    },
    { key: 'target', label: 'Compress to a target file size', kind: 'checkbox', default: false, control: true },
    {
      key: 'target_bytes',
      label: 'Target size',
      kind: 'number',
      default: 500000,
      show_if: { key: 'target', equals: 'true' },
    },
  ];
  const params = collectParams(compress, {
    quality: 80,
    scale: '100',
    target: true,
    target_bytes: 500000,
  });
  assert.deepEqual(params, { quality: 80, scale: 100, target_bytes: 500000 });
});

/**
 * Reduce colours is opt-in: unchecked, the lossless request must not carry `colours`
 * (the engine would switch to a lossy palette). Checked, the choice travels as a number.
 */
test('reduce colours only travels when it is switched on', () => {
  const compress: Field[] = [
    { key: 'reduce', label: 'Reduce colours (lossy)', kind: 'checkbox', default: false, control: true },
    {
      key: 'colours',
      label: 'Colours',
      kind: 'select',
      default: '256',
      options: ['256', '128', '64', '32', '16'],
      show_if: { key: 'reduce', equals: 'true' },
    },
  ];
  assert.deepEqual(collectParams(compress, { reduce: false, colours: '256' }), {});
  assert.deepEqual(collectParams(compress, { reduce: true, colours: '64' }), { colours: 64 });
});
