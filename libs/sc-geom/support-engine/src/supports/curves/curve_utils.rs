//! DragonFruit `src/supports/Curves/curveUtils.ts`.
//!
//! Two behaviours of the TypeScript are kept as they show in the result and
//! documented where they cannot be:
//!
//! - `updateCurvesAtJoint` mutates a segment that is already bezier in place
//!   (`ensureBezierSegment` returns the same object). The returned trunk sees
//!   every such write, including an `endTangent`/`startTangent` written before
//!   an unresolvable endpoint skipped the control points. The caller's own
//!   trunk object is not mutated here: Rust trunks are values.
//! - `removeSegmentCurve`/`removeCurveAtJoint` leave the segment's `bias` on the
//!   straight result (it is not in the destructured list). Nothing reads it, but
//!   it is kept (`SegmentShape::Straight::stray_bias`) so the record matches.

