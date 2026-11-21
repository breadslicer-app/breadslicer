//! A Rust transcription of `clipper-lib` 6.4.2.2, the JavaScript port (Timo,
//! 2013) of Angus Johnson's Clipper 6.4.2, as DragonFruit's raft code calls it:
//! the sweep-line boolean engine (`Clipper`) with `PolyTree` output and
//! `StrictlySimple`, and `ClipperOffset`.
//!
//! Original copyright: Angus Johnson 2010-2017; JavaScript port Timo 2013.
//! Use, modification & distribution is subject to the Boost Software License
//! Ver 1 (http://www.boost.org/LICENSE_1_0.txt).
//!
//! It follows the JavaScript, not the C#/C++: coordinates are doubles holding
//! integers, rounding is the V8 path's `R2`, the 128-bit products only engage
//! past `loRange` (47 453 132, not the C# 0x3FFFFFFF), an edge's duplicate
//! point test in `AddPath` compares objects (so never matches), and the one
//! JavaScript-only bug that throws (`ePrev2`) is an error here too.

// Clipper's own constants (6.28318530717959 is not TAU), branch shapes and
// clamps are kept as the JavaScript writes them: they are what parity is with.
#![allow(
    clippy::approx_constant,
    clippy::if_same_then_else,
    clippy::manual_clamp
)]

mod engine;
mod offset;

pub use engine::{Clipper, ClipperError, PolyNode, PolyTree};
pub use offset::ClipperOffset;

/// A point in Clipper's integer space, held as JavaScript holds it (a double).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct IntPoint {
    pub x: f64,
    pub y: f64,
}

impl IntPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        IntPoint { x, y }
    }
}

pub type Path = Vec<IntPoint>;
pub type Paths = Vec<Path>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipType {
    Intersection,
    Union,
    Difference,
    Xor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyType {
    Subject,
    Clip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyFillType {
    EvenOdd,
    NonZero,
    Positive,
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Square,
    Round,
    Miter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndType {
    OpenSquare,
    OpenRound,
    OpenButt,
    ClosedLine,
    ClosedPolygon,
}

/// `ClipperLib.Clipper.Round` on V8 (`R2`).
pub fn round(a: f64) -> f64 {
    if a < 0.0 {
        (a - 0.5).ceil()
    } else {
        (a + 0.5).floor()
    }
}

/// `ClipperLib.Cast_Int32` on V8 (`~~a`): ToInt32, truncating and wrapping.
pub fn cast_int32(a: f64) -> f64 {
    if !a.is_finite() {
        return 0.0;
    }
    let m = a.trunc().rem_euclid(4_294_967_296.0);
    if m >= 2_147_483_648.0 {
        m - 4_294_967_296.0
    } else {
        m
    }
}

/// `ClipperLib.Clipper.Area` of a path.
pub fn area(poly: &[IntPoint]) -> f64 {
    let cnt = poly.len();
    if cnt < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    let mut j = cnt - 1;
    for i in 0..cnt {
        a += (poly[j].x + poly[i].x) * (poly[j].y - poly[i].y);
        j = i;
    }
    -a * 0.5
}

/// `ClipperLib.Clipper.Orientation`.
pub fn orientation(poly: &[IntPoint]) -> bool {
    area(poly) >= 0.0
}
