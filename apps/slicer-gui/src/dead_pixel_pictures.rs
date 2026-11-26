/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The pictures of the dead pixels' steps (#783), as SVG: the printer's
//! screen in its own proportions, the sheet, the cut, the light through it
//! and the cells marked on the plate.
//!
//! The pictures carry no text, so they need no font: the words go beside
//! them, in the interface's language.

use std::fmt::Write;

/// A step of finding the dead pixels, and the picture that goes with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The sheet on the lit screen, a dead spot circled.
    Overview,
    /// The sheet printed, checked with a ruler.
    Print,
    /// The sheet cut along its dashed line.
    Cut,
    /// The sheet laid on the screen, the vat out, the front to the front.
    Lay,
    /// The screen lit behind the sheet, the dark spots circled.
    Light,
    /// The cells marked on the plate.
    Mark,
}

impl Step {
    pub(crate) const ALL: [Step; 6] = [
        Step::Overview,
        Step::Print,
        Step::Cut,
        Step::Lay,
        Step::Light,
        Step::Mark,
    ];
}

const GROUND: &str = "#14171a";
const BODY: &str = "#2b3034";
const EDGE: &str = "#4a5258";
const SCREEN_OFF: &str = "#1b1f22";
const LIGHT: &str = "#fff1bf";
const PAPER: &str = "#f4f1ec";
const PAPER_LIT: &str = "#fbf3d6";
const GRID: &str = "#cfc8bd";
const GRID_MAJOR: &str = "#a39a8e";
const INK: &str = "#2b3034";
const DEAD: &str = "#3a332b";
const MARK: &str = "#c85a40";
const AMBER: &str = "#e9a23b";
const TOOL: &str = "#e6e8ea";
const PLATE: &str = "#1c2024";
const PLATE_GRID: &str = "#2c3237";
const PLATE_MAJOR: &str = "#3b444b";
const ZONE: &str = "#b5523d";

/// The picture's size: 200 × 120.
const W: f32 = 200.0;
const H: f32 = 120.0;

struct Svg(String);

impl Svg {
    fn new() -> Self {
        Self(format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{}" height="{}">"#,
            W * 4.0,
            H * 4.0
        ))
    }

    fn rect(&mut self, [x, y, w, h]: [f32; 4], fill: &str, stroke: Option<(&str, f32)>, r: f32) {
        let stroke = stroke.map_or(String::new(), |(s, w)| {
            format!(r#" stroke="{s}" stroke-width="{w}""#)
        });
        let _ = write!(
            self.0,
            r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" rx="{r:.2}" fill="{fill}"{stroke}/>"#
        );
    }

    fn line(&mut self, a: [f32; 2], b: [f32; 2], stroke: &str, width: f32) {
        let _ = write!(
            self.0,
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{stroke}" stroke-width="{width}" stroke-linecap="round"/>"#,
            a[0], a[1], b[0], b[1]
        );
    }

    fn dashed(&mut self, [x, y, w, h]: [f32; 4], stroke: &str, width: f32) {
        let _ = write!(
            self.0,
            r#"<rect x="{x:.2}" y="{y:.2}" width="{w:.2}" height="{h:.2}" fill="none" stroke="{stroke}" stroke-width="{width}" stroke-dasharray="2.2 1.6"/>"#
        );
    }

    fn circle(&mut self, c: [f32; 2], r: f32, fill: &str, stroke: Option<(&str, f32)>) {
        let stroke = stroke.map_or(String::new(), |(s, w)| {
            format!(r#" stroke="{s}" stroke-width="{w}""#)
        });
        let _ = write!(
            self.0,
            r#"<circle cx="{:.2}" cy="{:.2}" r="{r:.2}" fill="{fill}"{stroke}/>"#,
            c[0], c[1]
        );
    }

    fn path(&mut self, d: &str, fill: &str, stroke: Option<(&str, f32)>) {
        let stroke = stroke.map_or(String::new(), |(s, w)| {
            format!(
                r#" stroke="{s}" stroke-width="{w}" stroke-linejoin="round" stroke-linecap="round""#
            )
        });
        let _ = write!(self.0, r#"<path d="{d}" fill="{fill}"{stroke}/>"#);
    }

    /// A soft glow round `[x, y, w, h]`.
    fn glow(&mut self, [x, y, w, h]: [f32; 4]) {
        for k in (1..=4).rev() {
            let o = k as f32 * 1.6;
            let _ = write!(
                self.0,
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{o:.2}" fill="{LIGHT}" fill-opacity="{:.2}"/>"#,
                x - o,
                y - o,
                w + 2.0 * o,
                h + 2.0 * o,
                0.07
            );
        }
    }

    fn finish(mut self) -> String {
        self.0.push_str("</svg>");
        self.0
    }
}

/// The box a screen of `aspect` (width over depth) takes in `room`
/// `[x, y, w, h]`, centred.
fn fitted(aspect: f32, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
    let aspect = aspect.clamp(0.4, 3.0);
    let (fw, fh) = if w / h > aspect {
        (h * aspect, h)
    } else {
        (w, w / aspect)
    };
    [x + (w - fw) / 2.0, y + (h - fh) / 2.0, fw, fh]
}

/// The sheet's grid in `[x, y, w, h]`, `n` cells across.
fn grid(svg: &mut Svg, [x, y, w, h]: [f32; 4], across: u32, minor: &str, major: &str) {
    let step = w / across as f32;
    let mut k = 1;
    while (k as f32) * step < w - 0.01 {
        let gx = x + k as f32 * step;
        let c = if k % 5 == 0 { major } else { minor };
        svg.line([gx, y], [gx, y + h], c, if k % 5 == 0 { 0.35 } else { 0.2 });
        k += 1;
    }
    let mut k = 1;
    while (k as f32) * step < h - 0.01 {
        let gy = y + h - k as f32 * step;
        let c = if k % 5 == 0 { major } else { minor };
        svg.line([x, gy], [x + w, gy], c, if k % 5 == 0 { 0.35 } else { 0.2 });
        k += 1;
    }
}

/// The printer seen from above: its body, and its screen's box.
fn printer(svg: &mut Svg, aspect: f32) -> [f32; 4] {
    svg.rect([30.0, 8.0, 140.0, 104.0], BODY, Some((EDGE, 0.8)), 6.0);
    // The arms of the Z axis, at the back.
    svg.rect([88.0, 10.0, 24.0, 8.0], EDGE, None, 1.5);
    let screen = fitted(aspect, [44.0, 24.0, 112.0, 74.0]);
    svg.rect(
        [
            screen[0] - 3.0,
            screen[1] - 3.0,
            screen[2] + 6.0,
            screen[3] + 6.0,
        ],
        GROUND,
        Some((EDGE, 0.6)),
        1.5,
    );
    screen
}

/// An arrow pointing down at `[x, y]`: the printer's front.
fn front_arrow(svg: &mut Svg, x: f32, y: f32) {
    svg.line([x, y - 6.0], [x, y], AMBER, 1.2);
    svg.path(
        &format!(
            "M{:.2} {:.2} L{:.2} {:.2} L{:.2} {:.2} Z",
            x - 2.4,
            y - 1.6,
            x + 2.4,
            y - 1.6,
            x,
            y + 1.6
        ),
        AMBER,
        None,
    );
}

/// Dark spots at shares `(u, v)` of `screen` (from its front left).
fn spots(screen: [f32; 4]) -> [[f32; 2]; 3] {
    let at = |u: f32, v: f32| [screen[0] + u * screen[2], screen[1] + screen[3] * (1.0 - v)];
    [at(0.28, 0.66), at(0.31, 0.62), at(0.72, 0.3)]
}

/// A pencil whose tip is at `tip`.
fn pencil(svg: &mut Svg, tip: [f32; 2]) {
    let [x, y] = tip;
    let d = |dx: f32, dy: f32| format!("{:.2} {:.2}", x + dx, y + dy);
    svg.path(
        &format!(
            "M{} L{} L{} L{} L{} Z",
            d(0.0, 0.0),
            d(3.2, -5.0),
            d(26.0, -26.0),
            d(30.0, -22.0),
            d(5.0, -1.8)
        ),
        TOOL,
        Some((INK, 0.5)),
    );
    svg.path(
        &format!("M{} L{} L{} Z", d(0.0, 0.0), d(3.2, -5.0), d(5.0, -1.8)),
        INK,
        None,
    );
}

/// Open scissors whose blades cross at `at`, cutting toward the left.
fn scissors(svg: &mut Svg, [x, y]: [f32; 2]) {
    let blade = |dy: f32| {
        format!(
            "M{:.2} {:.2} L{:.2} {:.2} L{:.2} {:.2} Z",
            x + 2.0,
            y + dy * 0.4,
            x - 12.0,
            y + dy * 1.6,
            x - 11.0,
            y + dy * 2.6
        )
    };
    svg.path(&blade(-1.0), TOOL, Some((INK, 0.5)));
    svg.path(&blade(1.0), TOOL, Some((INK, 0.5)));
    svg.line([x + 1.0, y - 0.6], [x + 6.5, y - 3.8], AMBER, 1.4);
    svg.line([x + 1.0, y + 0.6], [x + 6.5, y + 3.8], AMBER, 1.4);
    svg.circle([x + 9.0, y - 5.2], 3.0, "none", Some((AMBER, 1.4)));
    svg.circle([x + 9.0, y + 5.2], 3.0, "none", Some((AMBER, 1.4)));
    svg.circle([x, y], 0.8, INK, None);
}

/// The picture of `step`, for a screen of `aspect` (width over depth).
pub(crate) fn picture(step: Step, aspect: f32) -> String {
    let mut svg = Svg::new();
    svg.rect([0.0, 0.0, W, H], GROUND, None, 0.0);
    match step {
        Step::Overview | Step::Light => {
            let screen = printer(&mut svg, aspect);
            svg.glow(screen);
            svg.rect(screen, PAPER_LIT, Some((INK, 0.6)), 0.0);
            grid(&mut svg, screen, 20, GRID, GRID_MAJOR);
            let [a, b, c] = spots(screen);
            for p in [a, b, c] {
                svg.circle(p, 0.9, DEAD, None);
            }
            svg.circle(
                [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
                4.2,
                "none",
                Some((MARK, 0.7)),
            );
            if step == Step::Light {
                svg.circle(c, 3.4, "none", Some((MARK, 0.7)));
                pencil(&mut svg, [c[0] + 3.0, c[1] - 2.0]);
            }
            front_arrow(&mut svg, 100.0, 116.0);
        }
        Step::Print => {
            // The page, landscape, its heading and the screen on it.
            let page = [22.0, 10.0, 141.0, 100.0];
            svg.rect(page, PAPER, Some((EDGE, 0.5)), 1.0);
            svg.rect([30.0, 15.0, 42.0, 3.0], GRID_MAJOR, None, 0.5);
            svg.rect([30.0, 20.5, 28.0, 1.8], GRID, None, 0.5);
            // The ruler printed on the sheet.
            let (rx, ry, rw) = (113.0, 19.0, 42.0);
            svg.line([rx, ry], [rx + rw, ry], INK, 0.6);
            for k in 0..=10 {
                let x = rx + k as f32 * rw / 10.0;
                svg.line(
                    [x, ry],
                    [x, ry - if k % 2 == 0 { 2.0 } else { 1.1 }],
                    INK,
                    0.4,
                );
            }
            let screen = fitted(aspect, [36.0, 32.0, 113.0, 70.0]);
            svg.rect(screen, PAPER, Some((INK, 0.8)), 0.0);
            grid(&mut svg, screen, 20, GRID, GRID_MAJOR);
            // A real ruler on it, its marks lined up.
            svg.rect(
                [rx - 4.0, ry + 1.0, rw + 12.0, 7.0],
                AMBER,
                Some((INK, 0.4)),
                0.8,
            );
            for k in 0..=10 {
                let x = rx + k as f32 * rw / 10.0;
                svg.line(
                    [x, ry + 1.0],
                    [x, ry + if k % 2 == 0 { 5.0 } else { 3.4 }],
                    INK,
                    0.4,
                );
            }
            svg.circle([176.0, 21.0], 6.0, "#3f8f5a", None);
            svg.path(
                "M172.8 21.2 L175.2 23.6 L179.4 18.6",
                "none",
                Some((PAPER, 1.4)),
            );
        }
        Step::Cut => {
            let screen = fitted(aspect, [40.0, 22.0, 120.0, 78.0]);
            let band = 7.0;
            let cut = [
                screen[0] - band,
                screen[1] - band,
                screen[2] + 2.0 * band,
                screen[3] + 2.0 * band,
            ];
            svg.rect(
                [cut[0] - 8.0, cut[1] - 8.0, cut[2] + 16.0, cut[3] + 16.0],
                PAPER,
                Some((EDGE, 0.4)),
                1.0,
            );
            svg.rect(screen, PAPER, Some((INK, 0.9)), 0.0);
            grid(&mut svg, screen, 20, GRID, GRID_MAJOR);
            svg.dashed(cut, INK, 0.6);
            // The scissors on the dashed line, at the front right.
            scissors(&mut svg, [cut[0] + cut[2] * 0.7, cut[1] + cut[3]]);
        }
        Step::Lay => {
            let screen = printer(&mut svg, aspect);
            svg.rect(screen, SCREEN_OFF, None, 0.0);
            // The sheet on the screen, its edge on the screen's edge.
            svg.rect(screen, PAPER, Some((INK, 0.7)), 0.0);
            grid(&mut svg, screen, 20, GRID, GRID_MAJOR);
            // The vat, out beside the printer.
            svg.rect([176.0, 30.0, 20.0, 50.0], "none", Some((TOOL, 0.8)), 2.0);
            svg.path(
                "M164 55 L173 55 M170 52 L173 55 L170 58",
                "none",
                Some((TOOL, 0.8)),
            );
            front_arrow(&mut svg, 100.0, 116.0);
        }
        Step::Mark => {
            // The plate as the app draws it, cells marked.
            let plate = fitted(aspect, [24.0, 12.0, 152.0, 96.0]);
            svg.rect(plate, PLATE, Some((PLATE_MAJOR, 0.6)), 0.0);
            let across = 24u32;
            let cell = plate[2] / across as f32;
            for (c, r) in [(6, 9), (7, 9), (6, 10), (7, 10), (17, 4)] {
                let x = plate[0] + c as f32 * cell;
                let y = plate[1] + plate[3] - (r + 1) as f32 * cell;
                if y >= plate[1] {
                    svg.rect([x, y, cell, cell], ZONE, None, 0.0);
                }
            }
            grid(&mut svg, plate, across, PLATE_GRID, PLATE_MAJOR);
            // The cell under the cursor, and the cursor.
            let (hc, hr) = (17.0, 6.0);
            let hx = plate[0] + hc * cell;
            let hy = plate[1] + plate[3] - (hr + 1.0) * cell;
            svg.rect([hx, hy, cell, cell], "none", Some((AMBER, 0.8)), 0.0);
            let (cx, cy) = (hx + cell * 0.6, hy + cell * 0.6);
            svg.path(
                &format!(
                    "M{cx:.2} {cy:.2} L{:.2} {:.2} L{:.2} {:.2} L{:.2} {:.2} Z",
                    cx,
                    cy + 9.0,
                    cx + 2.4,
                    cy + 6.6,
                    cx + 6.2,
                    cy + 6.6
                ),
                TOOL,
                Some((INK, 0.5)),
            );
        }
    }
    svg.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_draws_for_wide_and_square_screens() {
        for step in Step::ALL {
            for aspect in [223.0 / 126.0, 1.0, 0.6] {
                let svg = picture(step, aspect);
                assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
                slint::Image::load_from_svg_data(svg.as_bytes()).expect("the picture reads");
            }
        }
    }

    /// With `BREADSLICER_DEAD_PIXEL_PICTURES` set to a folder, writes each
    /// picture there, to look at.
    #[test]
    fn the_pictures_are_written_when_asked() {
        let Ok(dir) = std::env::var("BREADSLICER_DEAD_PIXEL_PICTURES") else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();
        for (k, step) in Step::ALL.iter().enumerate() {
            let path = std::path::Path::new(&dir).join(format!("{k}-{step:?}.svg"));
            std::fs::write(path, picture(*step, 223.0 / 126.0)).unwrap();
        }
    }
}
