/**
 * Pure wheel geometry. No DOM access, no reactive state: every value returned
 * here is a plain function of its arguments so it can be unit tested with
 * `node --test`.
 *
 * Angles are radians, measured from the positive x axis, growing clockwise on
 * screen because SVG's y axis points down. The default start angle is -PI/2
 * (12 o'clock).
 */

export const TAU = Math.PI * 2;

export interface RingOptions {
  cx: number;
  cy: number;
  innerRadius: number;
  outerRadius: number;
  /** Width of the gap between neighbouring sectors, in viewBox units. */
  gapPx: number;
  startAngle?: number;
  /** Radius at which labels/paths are laid out. */
  labelRadius?: number;
}

export interface SectorGeometry {
  index: number;
  startAngle: number;
  endAngle: number;
  midAngle: number;
  /** Filled annular sector path. */
  path: string;
}

export interface RingGeometry {
  sectors: SectorGeometry[];
  count: number;
  step: number;
  gapAngle: number;
  startAngle: number;
  labelRadius: number;
}

export function normalizeAngle(angle: number): number {
  const a = angle % TAU;
  return a < 0 ? a + TAU : a;
}

export function polarToCartesian(
  cx: number,
  cy: number,
  radius: number,
  angle: number,
): { x: number; y: number } {
  return { x: cx + radius * Math.cos(angle), y: cy + radius * Math.sin(angle) };
}

const fmt = (n: number): string => (Math.round(n * 100) / 100).toString();

export function annulusSectorPath(
  cx: number,
  cy: number,
  innerRadius: number,
  outerRadius: number,
  a0: number,
  a1: number,
): string {
  const large = a1 - a0 > Math.PI ? 1 : 0;
  const o0 = polarToCartesian(cx, cy, outerRadius, a0);
  const o1 = polarToCartesian(cx, cy, outerRadius, a1);
  const i1 = polarToCartesian(cx, cy, innerRadius, a1);
  const i0 = polarToCartesian(cx, cy, innerRadius, a0);
  return [
    `M ${fmt(o0.x)} ${fmt(o0.y)}`,
    `A ${fmt(outerRadius)} ${fmt(outerRadius)} 0 ${large} 1 ${fmt(o1.x)} ${fmt(o1.y)}`,
    `L ${fmt(i1.x)} ${fmt(i1.y)}`,
    `A ${fmt(innerRadius)} ${fmt(innerRadius)} 0 ${large} 0 ${fmt(i0.x)} ${fmt(i0.y)}`,
    'Z',
  ].join(' ');
}

/**
 * A wedge whose side edges are exactly radial, with circular corners blending into
 * the arcs. Adjacent wedges therefore share one line direction — their facing edges
 * are parallel (collinear when the gap is zero), which is what makes a segmented
 * ring look machined rather than ragged.
 */
export function radialWedgePath(
  cx: number,
  cy: number,
  innerRadius: number,
  outerRadius: number,
  a0: number,
  a1: number,
  corner: number,
): string {
  const band = outerRadius - innerRadius;
  const span = a1 - a0;
  // The corner may not eat more than half the band or half the wedge's width.
  const limit = Math.min(band / 2 - 0.5, (span / 2) * innerRadius - 0.5);
  const c = Math.max(0.5, Math.min(corner, Math.max(0.5, limit)));

  const at = (r: number, a: number) => {
    const p = polarToCartesian(cx, cy, r, a);
    return `${fmt(p.x)} ${fmt(p.y)}`;
  };
  const dOut = c / outerRadius;
  const dIn = c / innerRadius;
  const large = a1 - a0 > Math.PI ? 1 : 0;

  return [
    // outer arc, from just past the start corner to just before the end corner
    `M ${at(outerRadius, a0 + dOut)}`,
    `A ${fmt(outerRadius)} ${fmt(outerRadius)} 0 ${large} 1 ${at(outerRadius, a1 - dOut)}`,
    `Q ${at(outerRadius, a1)} ${at(outerRadius - c, a1)}`,
    // radial edge inward
    `L ${at(innerRadius + c, a1)}`,
    `Q ${at(innerRadius, a1)} ${at(innerRadius, a1 - dIn)}`,
    `A ${fmt(innerRadius)} ${fmt(innerRadius)} 0 ${large} 0 ${at(innerRadius, a0 + dIn)}`,
    `Q ${at(innerRadius, a0)} ${at(innerRadius + c, a0)}`,
    // radial edge outward
    `L ${at(outerRadius - c, a0)}`,
    `Q ${at(outerRadius, a0)} ${at(outerRadius, a0 + dOut)}`,
    'Z',
  ].join(' ');
}


/**
 * One ring of `count` equal sectors, separated by `gapPx` measured on the mid
 * radius so the seam keeps the same width whatever the segment count.
 */
export function layoutRing(count: number, options: RingOptions): RingGeometry {
  const { cx, cy, innerRadius, outerRadius, gapPx } = options;
  const total = Math.max(1, Math.floor(count));
  const startAngle = options.startAngle ?? -Math.PI / 2;
  const labelRadius = options.labelRadius ?? innerRadius + (outerRadius - innerRadius) * 0.52;
  const step = TAU / total;
  const gapAngle = gapPx / Math.max(1, (innerRadius + outerRadius) / 2);
  const span = Math.max(0, step - gapAngle);

  const sectors: SectorGeometry[] = Array.from({ length: total }, (_, index) => {
    const a0 = startAngle + index * step + gapAngle / 2;
    const a1 = a0 + span;
    return {
      index,
      startAngle: a0,
      endAngle: a1,
      midAngle: startAngle + (index + 0.5) * step,
      path: annulusSectorPath(cx, cy, innerRadius, outerRadius, a0, a1),
    };
  });

  return { sectors, count: total, step, gapAngle, startAngle, labelRadius };
}

/** The sector under a point, or -1 when the point is off the annulus. */
export function indexForPoint(
  x: number,
  y: number,
  ring: RingGeometry,
  cx: number,
  cy: number,
  minRadius: number,
  maxRadius: number,
): number {
  const dx = x - cx;
  const dy = y - cy;
  const radius = Math.hypot(dx, dy);
  if (radius < minRadius || radius > maxRadius) return -1;
  const relative = normalizeAngle(Math.atan2(dy, dx) - ring.startAngle);
  const index = Math.floor(relative / ring.step);
  return Math.min(index, ring.count - 1);
}
