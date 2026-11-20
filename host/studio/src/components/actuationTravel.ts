/**
 * Key travel for fixed (traditional) actuation, in millimetres from the top of the key.
 *
 * A fixed key presses when it travels past its actuation point and, once held, releases when it
 * rises back to its release point. The release point sits above the actuation point (less travel);
 * the gap between them keeps a key resting near the actuation point from chattering. The daemon
 * stores the gap as the release depth in the actuation word.
 */

/** The deepest point of travel. */
export const TRAVEL_MAX_MM = 2.8;
/** The travel settings move in steps of this size. */
export const TRAVEL_STEP_MM = 0.1;
/** A fixed entry with no release point releases this far above its actuation point, as in the daemon. */
export const DEFAULT_RELEASE_GAP_MM = 0.2;

/** `mm` rounded to the nearest 0.1 mm step. */
export function travelStep(mm: number): number {
  return Math.round(mm * 10) / 10;
}

/** The release point a fixed entry uses; an entry without one releases 0.2 mm above its actuation point. */
export function releasePointOf(entry: { actuationPointMm: number; releasePointMm?: number }): number {
  return entry.releasePointMm ?? Math.max(0, travelStep(entry.actuationPointMm - DEFAULT_RELEASE_GAP_MM));
}

/** Why `releaseMm` can't be the release point for `actuationMm`, or `null` when it can. */
export function releasePointProblem(actuationMm: number, releaseMm: number): string | null {
  const act = travelStep(actuationMm);
  const rel = travelStep(releaseMm);
  if (rel < act) return null;
  return (
    `The release point (${rel.toFixed(1)} mm) must be above the actuation point (${act.toFixed(1)} mm). ` +
    `Choose less than ${act.toFixed(1)} mm of travel so a held key lets go before it can press again.`
  );
}

/**
 * The release point after the actuation point moves from `prevActuationMm` to `nextActuationMm`.
 * It keeps its gap above the actuation point; a release point that wasn't above uses the default gap.
 */
export function releaseAfterActuationMove(
  prevActuationMm: number,
  prevReleaseMm: number,
  nextActuationMm: number,
): number {
  const gap = travelStep(prevActuationMm - prevReleaseMm);
  const keep = gap > 0 ? gap : DEFAULT_RELEASE_GAP_MM;
  return Math.max(0, travelStep(nextActuationMm - keep));
}

/** A release point for `actuationMm`: `releaseMm` when it is above, otherwise the default gap above. */
export function validReleasePoint(actuationMm: number, releaseMm: number): number {
  return releasePointProblem(actuationMm, releaseMm) === null
    ? releaseMm
    : Math.max(0, travelStep(actuationMm - DEFAULT_RELEASE_GAP_MM));
}
