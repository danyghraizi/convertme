import test from 'node:test';
import assert from 'node:assert/strict';
import { extensionOf, isLosslessImage, isLosslessSelection } from './format.ts';

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

test('extensionOf handles directories and dots in the name', () => {
  assert.equal(extensionOf('/tmp/a.b/photo.JPEG'), 'jpeg');
  assert.equal(extensionOf('/tmp/photo'), '');
});
