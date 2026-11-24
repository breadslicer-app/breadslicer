/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The pictures of each reading step, as SVG: the coupon seen from above
//! with the features of the step lit, beside a close-up of how to read them
//! (calipers on a block, a drill bit in a hole, light through the slits…).
//!
//! The pictures carry no text, so they need no font: the words go beside
//! them, in the interface's language.

use std::fmt::Write;

use crate::coupon::{BAR_AT, BAR_SIZE, BASE_OUTLINE, Coupon, Feature};

/// A step of the reading, and the picture that goes with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The whole coupon, flat on the plate with no supports.
    Overview,
    /// Calipers across the blocks, along X.
    BlocksX,
    /// Calipers across the blocks, along Y.
    BlocksY,
    /// Drill bits into the blind holes.
    Holes,
    /// The pins that stood.
    Pins,
    /// The slits, against a light.
    Slits,
    /// The column on the plate: its width at the bottom and near the top.
    Foot,
}

const BASE: &str = "#2b3034";
const EDGE: &str = "#4a5258";
const PART: &str = "#5d666d";
const PIT: &str = "#14171a";
const LIT: &str = "#e9a23b";
const TOOL: &str = "#e6e8ea";
const LIGHT: &str = "#fff1bf";

/// The top view: model x from -3 to 79, y from -3 to 37 (flipped).
const TOP: [f32; 4] = [-3.0, -3.0, 82.0, 40.0];
/// The close-up, beside the top view.
const DETAIL_X: f32 = 84.0;
const DETAIL: f32 = 40.0;

struct Svg(String);

impl Svg {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: &str, stroke: Option<&str>) {
        let stroke = stroke.map_or(String::new(), |s| {
            format!(r#" stroke="{s}" stroke-width="0.3""#)
        });
        let _ = write!(
            self.0,
            r#"<rect x="{x:.3}" y="{y:.3}" width="{w:.3}" height="{h:.3}" fill="{fill}"{stroke}/>"#
        );
    }

    fn circle(&mut self, x: f32, y: f32, r: f32, fill: &str, stroke: Option<(&str, f32)>) {
        let stroke = stroke.map_or(String::new(), |(s, w)| {
            format!(r#" stroke="{s}" stroke-width="{w}""#)
        });
        let _ = write!(
            self.0,
            r#"<circle cx="{x:.3}" cy="{y:.3}" r="{r:.3}" fill="{fill}"{stroke}/>"#
        );
    }

    fn line(&mut self, a: [f32; 2], b: [f32; 2], stroke: &str, width: f32) {
        let _ = write!(
            self.0,
            r#"<line x1="{:.3}" y1="{:.3}" x2="{:.3}" y2="{:.3}" stroke="{stroke}" stroke-width="{width}" stroke-linecap="round"/>"#,
            a[0], a[1], b[0], b[1]
        );
    }

    fn polygon(&mut self, points: &[[f32; 2]], fill: &str) {
        let points: Vec<String> = points
            .iter()
            .map(|p| format!("{:.3},{:.3}", p[0], p[1]))
            .collect();
        let _ = write!(
            self.0,
            r#"<polygon points="{}" fill="{fill}"/>"#,
            points.join(" ")
        );
    }

    /// A dimension line from `a` to `b` with arrowheads.
    fn dimension(&mut self, a: [f32; 2], b: [f32; 2], stroke: &str) {
        self.line(a, b, stroke, 0.25);
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-6);
        let (ux, uy) = (d[0] / len, d[1] / len);
        let (head, half) = (0.9, 0.45);
        for (tip, sign) in [(a, 1.0), (b, -1.0)] {
            let back = [tip[0] + sign * ux * head, tip[1] + sign * uy * head];
            self.polygon(
                &[
                    tip,
                    [back[0] - uy * half, back[1] + ux * half],
                    [back[0] + uy * half, back[1] - ux * half],
                ],
                stroke,
            );
        }
    }
}

/// From model coordinates to the top view's.
fn top(p: [f32; 2]) -> [f32; 2] {
    [p[0] - TOP[0], TOP[1] + TOP[3] - p[1]]
}

fn top_rect(svg: &mut Svg, at: [f32; 2], size: [f32; 2], fill: &str, stroke: Option<&str>) {
    let corner = top([at[0], at[1] + size[1]]);
    svg.rect(corner[0], corner[1], size[0], size[1], fill, stroke);
}

/// The picture of a step.
pub fn picture(coupon: &Coupon, step: Step) -> String {
    let width = DETAIL_X + DETAIL;
    let height = TOP[3];
    let mut svg = Svg(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {width} {height}">"#,
        (width * 10.0) as u32,
        (height * 10.0) as u32
    ));
    top_view(&mut svg, coupon, step);
    let _ = write!(svg.0, r#"<g transform="translate({DETAIL_X} 0)">"#);
    detail(&mut svg, step);
    svg.0.push_str("</g></svg>");
    svg.0
}

fn top_view(svg: &mut Svg, coupon: &Coupon, step: Step) {
    svg.polygon(&BASE_OUTLINE.map(top), BASE);
    // The point of the base shows +X.
    let point = [[61.0, 4.0], [66.0, 17.0], [61.0, 30.0]].map(top);
    svg.polygon(&point, if step == Step::Overview { LIT } else { EDGE });

    // The bar of holes.
    top_rect(svg, BAR_AT, BAR_SIZE, PART, None);
    for hole in &coupon.holes {
        if let Feature::Hole { at, diameter } = *hole {
            let c = top(at);
            let ring = (step == Step::Holes).then_some((LIT, 0.35));
            svg.circle(c[0], c[1], diameter / 2.0, PIT, ring);
        }
    }

    let blocks_lit = matches!(step, Step::BlocksX | Step::BlocksY);
    for block in &coupon.blocks {
        if let Feature::Block { at, side } = *block {
            top_rect(
                svg,
                at,
                [side, side],
                if blocks_lit { LIT } else { PART },
                None,
            );
            match step {
                Step::BlocksX => {
                    let y = at[1] - 1.3;
                    svg.dimension(top([at[0], y]), top([at[0] + side, y]), TOOL);
                }
                Step::BlocksY => {
                    let x = at[0] - 1.3;
                    svg.dimension(top([x, at[1]]), top([x, at[1] + side]), TOOL);
                }
                _ => {}
            }
        }
    }

    for pin in &coupon.pins {
        if let Feature::Pin { at, diameter } = *pin {
            let c = top(at);
            let fill = if step == Step::Pins { LIT } else { PART };
            svg.circle(c[0], c[1], diameter / 2.0, fill, None);
        }
    }

    for tooth in &coupon.teeth {
        if let Feature::Tooth { at, size } = *tooth {
            let fill = if step == Step::Slits { LIT } else { PART };
            top_rect(svg, at, size, fill, None);
        }
    }

    if let Feature::Column { at, side } = coupon.column {
        let fill = if step == Step::Foot { LIT } else { PART };
        top_rect(svg, at, [side, side], fill, Some(EDGE));
        if step == Step::Foot {
            let y = at[1] - 1.3;
            svg.dimension(top([at[0], y]), top([at[0] + side, y]), TOOL);
        }
    }
}

fn plate(svg: &mut Svg) {
    svg.rect(1.0, 34.0, DETAIL - 2.0, 2.5, EDGE, None);
}

/// Calipers seen from the front, jaws closed on `[left, right]`, the work
/// standing from `top` to the plate.
fn calipers(svg: &mut Svg, left: f32, right: f32, top: f32) {
    let beam_y = top - 12.0;
    svg.rect(2.0, beam_y, 36.0, 3.0, TOOL, None);
    // The slider and its thumb wheel.
    svg.rect(right - 2.0, beam_y - 2.0, 7.0, 7.0, TOOL, None);
    svg.circle(right + 1.5, beam_y + 7.0, 1.2, TOOL, None);
    // The jaws reach past the top of the work and close on its sides.
    svg.polygon(
        &[
            [left - 2.5, beam_y + 3.0],
            [left, beam_y + 3.0],
            [left, top + 6.0],
            [left - 1.0, top + 7.0],
            [left - 2.5, beam_y + 6.0],
        ],
        TOOL,
    );
    svg.polygon(
        &[
            [right, beam_y + 3.0],
            [right + 2.5, beam_y + 3.0],
            [right + 2.5, beam_y + 6.0],
            [right + 1.0, top + 7.0],
            [right, top + 6.0],
        ],
        TOOL,
    );
}

fn detail(svg: &mut Svg, step: Step) {
    match step {
        Step::Overview => {
            plate(svg);
            // The coupon flat on the plate, and the column beside it.
            svg.rect(4.0, 31.0, 24.0, 3.0, PART, None);
            for x in [6.0, 11.0, 17.0, 22.0] {
                svg.rect(x, 27.0, 3.5, 4.0, PART, None);
            }
            svg.rect(31.0, 25.0, 5.0, 9.0, PART, None);
            // No supports: a crossed-out pillar.
            svg.rect(17.0, 8.0, 3.0, 12.0, EDGE, None);
            svg.line([13.0, 8.0], [24.0, 20.0], LIT, 1.0);
            svg.line([24.0, 8.0], [13.0, 20.0], LIT, 1.0);
        }
        Step::BlocksX | Step::BlocksY => {
            plate(svg);
            svg.rect(4.0, 31.0, 32.0, 3.0, BASE, Some(EDGE));
            svg.rect(12.0, 21.0, 16.0, 10.0, LIT, None);
            calipers(svg, 12.0, 28.0, 21.0);
        }
        Step::Holes => {
            plate(svg);
            // The bar cut through a hole: the bit goes into the pit.
            svg.rect(4.0, 22.0, 32.0, 12.0, PART, None);
            svg.rect(17.5, 22.0, 5.0, 10.0, PIT, None);
            svg.line([17.5, 22.0], [17.5, 32.0], LIT, 0.5);
            svg.line([22.5, 22.0], [22.5, 32.0], LIT, 0.5);
            svg.line([17.5, 32.0], [22.5, 32.0], LIT, 0.5);
            // The drill bit, held by hand: shank above, flutes going in.
            svg.rect(18.3, 2.0, 3.4, 12.0, TOOL, None);
            svg.polygon(
                &[
                    [18.3, 14.0],
                    [21.7, 14.0],
                    [21.7, 29.0],
                    [20.0, 31.0],
                    [18.3, 29.0],
                ],
                TOOL,
            );
            for k in 0..6 {
                let y = 15.0 + 2.4 * k as f32;
                svg.line([18.3, y + 1.6], [21.7, y], PART, 0.35);
            }
        }
        Step::Pins => {
            plate(svg);
            svg.rect(2.0, 31.0, 36.0, 3.0, BASE, Some(EDGE));
            // The thinnest broke off and lies on the base; the next stands.
            svg.rect(7.6, 28.0, 0.8, 3.0, PART, None);
            svg.polygon(&[[2.4, 31.0], [2.6, 30.2], [7.2, 30.2], [7.0, 31.0]], PART);
            svg.rect(17.2, 17.0, 1.6, 14.0, LIT, None);
            svg.rect(27.7, 17.0, 2.6, 14.0, PART, None);
        }
        Step::Slits => {
            // Teeth seen from above, close: light comes through the open
            // slits; the thinnest one closed.
            svg.circle(20.0, 3.5, 2.5, LIGHT, None);
            let gaps = [0.8, 1.2, 1.6, 2.4];
            let mut x = 4.0;
            for (k, gap) in gaps.iter().chain(std::iter::once(&0.0)).enumerate() {
                svg.rect(x, 12.0, 5.0, 22.0, PART, None);
                x += 5.0;
                if k < gaps.len() {
                    if k == 0 {
                        svg.rect(x, 12.0, *gap, 22.0, PART, None);
                    } else {
                        svg.rect(x, 12.0, *gap, 22.0, LIGHT, None);
                        if k == 1 {
                            svg.line([x, 12.0], [x, 34.0], LIT, 0.4);
                            svg.line([x + gap, 12.0], [x + gap, 34.0], LIT, 0.4);
                        }
                        svg.line([20.0, 6.0], [x + gap / 2.0, 12.0], LIGHT, 0.25);
                    }
                    x += gap;
                }
            }
        }
        Step::Foot => {
            plate(svg);
            // The column from the side: its first layers spread.
            svg.polygon(
                &[
                    [11.0, 34.0],
                    [12.0, 32.4],
                    [14.0, 31.0],
                    [14.0, 12.0],
                    [26.0, 12.0],
                    [26.0, 31.0],
                    [28.0, 32.4],
                    [29.0, 34.0],
                ],
                PART,
            );
            svg.dimension([11.0, 37.0], [29.0, 37.0], LIT);
            svg.dimension([14.0, 9.5], [26.0, 9.5], TOOL);
            svg.line([11.0, 34.0], [11.0, 37.6], LIT, 0.2);
            svg.line([29.0, 34.0], [29.0, 37.6], LIT, 0.2);
            svg.line([14.0, 12.0], [14.0, 8.9], TOOL, 0.2);
            svg.line([26.0, 12.0], [26.0, 8.9], TOOL, 0.2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_draws_a_whole_picture() {
        let coupon = Coupon::xy();
        for step in [
            Step::Overview,
            Step::BlocksX,
            Step::BlocksY,
            Step::Holes,
            Step::Pins,
            Step::Slits,
            Step::Foot,
        ] {
            let svg = picture(&coupon, step);
            assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
            assert!(!svg.contains("NaN"), "{step:?}");
            assert!(svg.contains(LIT), "{step:?} lights nothing");
        }
    }
}

#[cfg(test)]
mod pictures {
    /// With `BREADSLICER_CALIBRATION_PICTURES=<folder>`, writes each step's
    /// picture there, to look at.
    #[test]
    #[ignore]
    fn the_pictures_are_written_when_asked() {
        let Ok(folder) = std::env::var("BREADSLICER_CALIBRATION_PICTURES") else {
            return;
        };
        let coupon = crate::Coupon::xy();
        std::fs::create_dir_all(&folder).unwrap();
        for step in [
            super::Step::Overview,
            super::Step::BlocksX,
            super::Step::BlocksY,
            super::Step::Holes,
            super::Step::Pins,
            super::Step::Slits,
            super::Step::Foot,
        ] {
            let path = std::path::Path::new(&folder).join(format!("{step:?}.svg"));
            std::fs::write(path, super::picture(&coupon, step)).unwrap();
        }
        std::fs::write(
            std::path::Path::new(&folder).join("coupon.stl"),
            coupon.to_stl(),
        )
        .unwrap();
    }
}
