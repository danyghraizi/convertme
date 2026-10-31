import test from 'node:test';
import assert from 'node:assert/strict';
import {
  extensionOf,
  formatTimecode,
  isLosslessImage,
  isLosslessSelection,
  isPngSelection,
  unreachableTarget,
} from './format.ts';

test('lossless image extensions are recognised, case-insensitively', () => {
  for (const name of ['a.png', 'b.PNG', 'c.bmp', 'd.tif', 'e.tiff', 'f.svg', 'g.ico']) {
    assert.equal(isLosslessImage(name), true, name);
  }
  for (const name of ['a.jpg', 'b.jpeg', 'c.webp', 'd.avif', 'e.heic', 'f.heif', 'no-ext']) {
    assert.equal(isLosslessImage(name), false, name);
  }
});

test('quality is hidden only when every selected file is lossless', () => {
  assert.equal(isLosslessSelection(['/a/poster.png', '/a/icon.bmp']), true);
  assert.equal(isLosslessSelection(['/a/poster.png', '/a/photo.jpg']), false);
  assert.equal(isLosslessSelection([]), false);
});

test('reduce colours is offered only for an all-PNG selection', () => {
  assert.equal(isPngSelection(['/a/poster.png', '/b/icon.PNG']), true);
  assert.equal(isPngSelection(['/a/poster.png', '/a/photo.jpg']), false);
  assert.equal(isPngSelection(['/a/photo.jpg']), false);
  assert.equal(isPngSelection([]), false);
});

test('the preview says when a target is out of reach', () => {
  assert.equal(unreachableTarget(42_000, { target_bytes: 1_000 }), 1_000);
  assert.equal(unreachableTarget(1_000, { target_bytes: 1_000 }), null);
  assert.equal(unreachableTarget(900, { target_bytes: 1_000 }), null);
  assert.equal(unreachableTarget(42_000, {}), null);
  assert.equal(unreachableTarget(42_000, { target_bytes: 0 }), null);
});

test('extensionOf handles directories and dots in the name', () => {
  assert.equal(extensionOf('/tmp/a.b/photo.JPEG'), 'jpeg');
  assert.equal(extensionOf('/tmp/photo'), '');
});

test('a timeline readout never prints 60.00 or a negative time', () => {
  assert.equal(formatTimecode(0), '0:00.00');
  assert.equal(formatTimecode(2.5), '0:02.50');
  assert.equal(formatTimecode(59.999), '1:00.00');
  assert.equal(formatTimecode(61.25), '1:01.25');
  assert.equal(formatTimecode(-3), '0:00.00');
  assert.equal(formatTimecode(Number.NaN), '0:00.00');
  assert.equal(formatTimecode(600), '10:00.00');
});
