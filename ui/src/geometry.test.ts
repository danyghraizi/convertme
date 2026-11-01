import test from 'node:test';
import assert from 'node:assert/strict';
import {
  TAU,
  backgroundWindow,
  collageCellBox,
  indexForPoint,
  layoutRing,
  polarToCartesian,
  radialWedgePath,
  type RingOptions,
} from './geometry.ts';

const BASE: RingOptions = {
  cx: 320,
  cy: 320,
  innerRadius: 150,
  outerRadius: 296,
  gapPx: 2,
};


test('sector spans plus gaps cover exactly one full turn', () => {
  for (const count of [1, 2, 3, 7, 9, 12, 24]) {
    const ring = layoutRing(count, BASE);
    assert.equal(ring.sectors.length, count);
    let covered = 0;
    for (const sector of ring.sectors) {
      assert.ok(sector.endAngle > sector.startAngle, `count ${count}: empty sector`);
      covered += sector.endAngle - sector.startAngle + ring.gapAngle;
    }
    assert.ok(Math.abs(covered - TAU) < 1e-9, `count ${count}: covered ${covered}`);
  }
});

test('gaps are at least two units wide at the ring mid radius', () => {
  for (const count of [1, 3, 7, 9, 12]) {
    const ring = layoutRing(count, BASE);
    const midRadius = (BASE.innerRadius + BASE.outerRadius) / 2;
    assert.ok(
      ring.gapAngle * midRadius >= 2 - 1e-9,
      `count ${count}: gap ${ring.gapAngle * midRadius}`,
    );
    assert.ok(ring.gapAngle > 0);
  }
});

test('mid angles are centred inside their sector', () => {
  for (const count of [3, 7, 9, 12]) {
    const ring = layoutRing(count, BASE);
    ring.sectors.forEach((sector, index) => {
      assert.ok(
        Math.abs(sector.midAngle - (ring.startAngle + (index + 0.5) * ring.step)) < 1e-9,
      );
      assert.ok(sector.midAngle > sector.startAngle);
      assert.ok(sector.midAngle < sector.endAngle);
    });
  }
});

test('indexForPoint selects the sector under each mid angle', () => {
  for (const count of [3, 7, 9, 12]) {
    const ring = layoutRing(count, BASE);
    for (const sector of ring.sectors) {
      const p = polarToCartesian(BASE.cx, BASE.cy, 240, sector.midAngle);
      assert.equal(
        indexForPoint(p.x, p.y, ring, BASE.cx, BASE.cy, BASE.innerRadius, BASE.outerRadius),
        sector.index,
        `count ${count} sector ${sector.index}`,
      );
    }
  }
});

test('points outside the annulus select nothing', () => {
  const ring = layoutRing(9, BASE);
  assert.equal(
    indexForPoint(BASE.cx, BASE.cy, ring, BASE.cx, BASE.cy, BASE.innerRadius, BASE.outerRadius),
    -1,
  );
  assert.equal(
    indexForPoint(
      BASE.cx + BASE.outerRadius + 40,
      BASE.cy,
      ring,
      BASE.cx,
      BASE.cy,
      BASE.innerRadius,
      BASE.outerRadius,
    ),
    -1,
  );
});

test('sector paths are closed annulus shapes', () => {
  const ring = layoutRing(7, BASE);
  for (const sector of ring.sectors) {
    assert.match(sector.path, /^M /);
    assert.ok(sector.path.includes(' A '));
    assert.ok(sector.path.trimEnd().endsWith('Z'));
  }
});

test('single segment ring stays a valid arc', () => {
  const ring = layoutRing(1, BASE);
  const [only] = ring.sectors;
  assert.ok(only.endAngle - only.startAngle > Math.PI);
  const largeArc = only.path.includes(' 1 1 ');
  assert.ok(largeArc, 'single sector uses the large-arc flag');
});

test('radial wedges keep radial side edges and rounded corners', () => {
  for (const count of [3, 7, 11, 12]) {
    const step = (2 * Math.PI) / count;
    for (let i = 0; i < count; i += 1) {
      const a0 = -Math.PI / 2 + i * step;
      const d = radialWedgePath(320, 320, 140, 300, a0, a0 + step - 0.01, 16);
      assert.ok(d.startsWith('M '), d);
      assert.ok(d.endsWith('Z'), d);
      assert.equal(d.match(/A /g)?.length, 2, d);
      assert.equal(d.match(/Q /g)?.length, 4, d);
      assert.equal(d.match(/L /g)?.length, 2, d);
      assert.ok(!d.includes('NaN'), d);
    }
  }
});

test('neighbouring wedge edges lie on the same radial line', () => {
  // With a zero gap the two facing edges are collinear, so the seam is one line.
  const a0 = 0.4;
  const first = radialWedgePath(0, 0, 100, 200, a0 - 0.5, a0, 10);
  const second = radialWedgePath(0, 0, 100, 200, a0, a0 + 0.5, 10);
  const linePoints = (d: string) =>
    [...d.matchAll(/L ([\d.-]+) ([\d.-]+)/g)].map((m) => [Number(m[1]), Number(m[2])]);

  // `first` ends at a0: its first L is inner+corner on the a0 ray.
  // `second` starts at a0: its second L is outer-corner on the a0 ray.
  const innerEdge = linePoints(first)[0];
  const outerEdge = linePoints(second)[1];
  for (const [x, y] of [innerEdge, outerEdge]) {
    // the path rounds to 0.01, so the cross product is only accurate to ~0.05
    assert.ok(Math.abs(x * Math.sin(a0) - y * Math.cos(a0)) < 0.05, `not radial: ${x},${y}`);
  }
  const cross = innerEdge[0] * outerEdge[1] - innerEdge[1] * outerEdge[0];
  // rounding to 0.01 at r=190 leaves a cross product around 0.3
  assert.ok(Math.abs(cross) < 1, `edges not collinear: ${cross}`);
});

test('background window starts at the cover offset, not the source origin', () => {
  // A 2:1 picture on a square canvas: the cover window is 100x100 at x=50.
  const win = backgroundWindow(200, 100, 100, 100, 1, 0, 0)!;
  const sourceX = win.baseX + win.cropX;
  assert.equal(win.baseX, 50);
  assert.equal(win.baseY, 0);
  assert.equal(win.visW, 100);
  assert.equal(sourceX, 50, 'a 1:1 crop of a 2:1 source starts halfway across');
  assert.equal(win.baseX + win.cropX + win.visW, 150, 'and is 100 wide');
});

test('background pan moves the visible window inside the cover', () => {
  // zoom 2 leaves a 50 px band: pan +100 pins the window to the left edge.
  const centre = backgroundWindow(200, 100, 100, 100, 2, 0, 0)!;
  const right = backgroundWindow(200, 100, 100, 100, 2, 100, 0)!;
  const left = backgroundWindow(200, 100, 100, 100, 2, -100, 0)!;
  assert.equal(centre.baseX + centre.cropX, 75);
  assert.equal(right.baseX + right.cropX, 50);
  assert.equal(left.baseX + left.cropX, 100);
});

test('a collage cell box matches the engine pan at both ends', () => {
  // The engine test: 200x100 red/blue source, 1024 cell, zoom 2, sample at x=256.
  // In display units (cell 512): one source px is 10.24 display px.
  const centred = collageCellBox(512, 512, 200, 100, 2, 0, 0, 'crop')!;
  const panned = collageCellBox(512, 512, 200, 100, 2, 100, 0, 'crop')!;
  const scale = centred.width / 200;
  const sample = (box: typeof centred, x: number) => (x - box.left) / scale;
  assert.equal(centred.width, 2048);
  assert.equal(centred.height, 1024);
  assert.ok(sample(centred, 128) < 100, 'centred shows the red half');
  assert.ok(sample(panned, 128) > 100, 'pan +100 shows the blue half');
  const contained = collageCellBox(512, 512, 100, 200, 1, 0, 0, 'contain')!;
  assert.equal(contained.width, 256, 'contain keeps the source aspect');
  assert.equal(contained.height, 512);
  assert.equal(contained.left, 128, 'and centres the picture in the cell');
});
