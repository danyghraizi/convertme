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
