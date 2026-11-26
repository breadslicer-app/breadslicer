/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The screen sheet (#783): the printer's screen at its real size, with the
//! plate's grid in 5 mm cells named like a map, to print, cut, lay on the
//! screen and see the dead pixels through it with the printer's cleaning
//! test on. A screen wider than a page is split over pages that butt
//! together along grid lines.
//!
//! Everything is laid out in millimetres, y down from the page's top left;
//! the screen's x runs from its left edge and y from its front.

use krilla::Document;
use krilla::geom::Transform;
use krilla::page::PageSettings;
use krilla::paint::{LineCap, StrokeDash};

use super::pdf::{
    Faces, INK, INK_SOFT, MUTED, PAPER, PT, Pen, Rgb, fill, fit, spaced_width, stroke,
};
use crate::dead_zones::{CELL_MM, DeadZone, column_letters};

/// A4, in portrait.
const A4: (f32, f32) = (210.0, 297.0);
/// What most printers leave unprinted round a page, and a hair more.
const MARGIN: f32 = 8.0;
/// The heading's height, over the screen.
const HEADING: f32 = 21.0;
/// The strip round the screen's edge that carries the cells' names and the
/// front, inside the line to cut along.
const BAND: f32 = 7.0;

const CELL_LINE: Rgb = (0xdd, 0xd8, 0xd0);
const TEN_LINE: Rgb = (0xc4, 0xbc, 0xb0);
const DEAD: Rgb = (0xf2, 0xc4, 0xb8);
const DEAD_EDGE: Rgb = (0xc8, 0x5a, 0x40);

/// The words of the sheet, in the app's language.
#[derive(Clone, Debug)]
pub(crate) struct ScreenTexts {
    pub title: String,
    pub front: String,
    /// Under the ruler: print it at its real size.
    pub actual_size: String,
    pub cut: String,
    /// `{page}` of `{pages}`.
    pub page: String,
    /// On a page of a screen split over several: butt its cut edges.
    pub join: String,
}

impl Default for ScreenTexts {
    fn default() -> Self {
        Self {
            title: "Dead pixel sheet".into(),
            front: "FRONT".into(),
            actual_size: "Print at actual size (100 %): this ruler measures 50 mm.".into(),
            cut: "Cut along the dashed line. Lay the sheet on the screen with the vat out, its thick edge on the lit edge of the screen, and run the printer's cleaning or exposure test: the dead pixels show dark.".into(),
            page: "Page {page} of {pages}".into(),
            join: "Cut the edges with no band exactly on their line and butt them against the next page.".into(),
        }
    }
}

/// Everything the sheet shows.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScreenSheet {
    pub printer: String,
    pub version: String,
    /// The screen, mm: X across, Y front to back.
    pub width_mm: f32,
    pub depth_mm: f32,
    /// The zones already marked, mm from the screen's middle.
    pub zones: Vec<DeadZone>,
    pub texts: ScreenTexts,
}

/// A part of the screen on one page: its x from the left edge and y from
/// the front, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Piece {
    pub x: [f32; 2],
    pub y: [f32; 2],
}

/// How the screen lies on paper: the page's size and its pieces, one a
/// page, left to right then front to back.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub page: (f32, f32),
    pub pieces: Vec<Piece>,
}

/// The room a page leaves for the screen: across and down.
fn room((w, h): (f32, f32)) -> (f32, f32) {
    (
        w - 2.0 * (MARGIN + BAND),
        h - 2.0 * (MARGIN + BAND) - HEADING,
    )
}

/// Where `length` mm splits so that each part fits `room`: at 10 mm lines,
/// as even as they come.
fn splits(length: f32, room: f32) -> Vec<[f32; 2]> {
    let step10 = |v: f32| (v / 10.0).ceil() * 10.0;
    let mut count = (length / room).ceil().max(1.0);
    let mut step = step10(length / count);
    if count > 1.0 && step > room {
        step = (room / 10.0).floor().max(1.0) * 10.0;
        count = (length / step).ceil();
    }
    if count <= 1.0 {
        return vec![[0.0, length]];
    }
    let mut out = Vec::new();
    let mut at = 0.0;
    while at < length - 0.01 {
        let end = (at + step).min(length);
        out.push([at, end]);
        at = end;
    }
    out
}

/// How a screen of `width` × `depth` mm lies on A4: portrait or landscape,
/// whichever takes fewer pages (landscape on a tie).
pub(crate) fn layout(width: f32, depth: f32) -> Layout {
    let try_page = |page: (f32, f32)| {
        let (rw, rh) = room(page);
        let xs = splits(width, rw);
        let ys = splits(depth, rh);
        let pieces: Vec<Piece> = ys
            .iter()
            .flat_map(|y| xs.iter().map(move |x| Piece { x: *x, y: *y }))
            .collect();
        Layout { page, pieces }
    };
    let landscape = try_page((A4.1, A4.0));
    let portrait = try_page(A4);
    if portrait.pieces.len() < landscape.pieces.len() {
        portrait
    } else {
        landscape
    }
}

/// The PDF of `sheet`.
pub(crate) fn write(sheet: &ScreenSheet) -> Result<Vec<u8>, String> {
    let faces = Faces::new();
    let mut document = Document::new();
    document.set_metadata(
        krilla::metadata::Metadata::new()
            .title(format!("{} · {}", sheet.texts.title, sheet.printer))
            .creator(format!("Breadslicer {}", sheet.version))
            .producer(format!("Breadslicer {}", sheet.version)),
    );
    let layout = layout(sheet.width_mm, sheet.depth_mm);
    let (pw, ph) = layout.page;
    let size = PageSettings::from_wh(pw / PT, ph / PT).ok_or("page size")?;
    let pages = layout.pieces.len();
    for (index, piece) in layout.pieces.iter().enumerate() {
        let mut page = document.start_page_with(size.clone());
        let mut surface = page.surface();
        surface.push_transform(&Transform::from_scale(1.0 / PT, 1.0 / PT));
        {
            let mut pen = Pen {
                s: &mut surface,
                faces: &faces,
            };
            pen.rect(0.0, 0.0, pw, ph, Some(fill(PAPER)), None);
            heading(&mut pen, sheet, layout.page, index, pages);
            screen(&mut pen, sheet, layout.page, *piece);
        }
        surface.pop();
        surface.finish();
        page.finish();
    }
    document.finish().map_err(|err| format!("{err:?}"))
}

fn heading(pen: &mut Pen, sheet: &ScreenSheet, (pw, _): (f32, f32), index: usize, pages: usize) {
    let f = pen.faces;
    let (left, right) = (MARGIN, pw - MARGIN);
    let ruler_left = right - 50.0;
    pen.text(
        &f.bold,
        12.0 * PT,
        left,
        MARGIN + 4.2,
        &sheet.texts.title,
        INK,
    );
    let printer = format!(
        "{} · {} × {} mm",
        sheet.printer,
        trim(sheet.width_mm),
        trim(sheet.depth_mm)
    );
    pen.text(
        &f.medium,
        7.5 * PT,
        left,
        MARGIN + 8.6,
        &fit(&f.medium, 7.5 * PT, &printer, ruler_left - left - 6.0),
        INK_SOFT,
    );
    if pages > 1 {
        let text = sheet
            .texts
            .page
            .replace("{page}", &(index + 1).to_string())
            .replace("{pages}", &pages.to_string());
        pen.text_right(&f.medium, 7.0 * PT, right, MARGIN + 3.6, &text, INK_SOFT);
    }
    // The ruler, to check the paper came out at its real size.
    let y = MARGIN + 8.0;
    pen.line([ruler_left, y], [right, y], stroke(INK, 0.6 * PT));
    for k in 0..=10 {
        let x = ruler_left + k as f32 * 5.0;
        let len = if k % 2 == 0 { 2.2 } else { 1.2 };
        pen.line([x, y], [x, y - len], stroke(INK, 0.45 * PT));
    }
    pen.text_centre(&f.regular, 5.5 * PT, ruler_left, y + 2.6, "0", MUTED);
    pen.text_centre(&f.regular, 5.5 * PT, right, y + 2.6, "50 mm", MUTED);
    pen.text(
        &f.regular,
        6.0 * PT,
        left,
        MARGIN + 12.6,
        &fit(
            &f.regular,
            6.0 * PT,
            &sheet.texts.actual_size,
            pw - 2.0 * MARGIN,
        ),
        INK_SOFT,
    );
    let mut lines = wrap(f, 6.0 * PT, &sheet.texts.cut, pw - 2.0 * MARGIN);
    if pages > 1 {
        lines.extend(wrap(f, 6.0 * PT, &sheet.texts.join, pw - 2.0 * MARGIN));
    }
    for (k, line) in lines.iter().take(2).enumerate() {
        pen.text(
            &f.regular,
            6.0 * PT,
            left,
            MARGIN + 15.6 + k as f32 * 2.7,
            line,
            MUTED,
        );
    }
}

/// `text` broken into lines of at most `width` mm at `size`.
fn wrap(f: &Faces, size: f32, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let next = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if f.regular.width(&next, size) > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn trim(mm: f32) -> String {
    let s = format!("{mm:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn screen(pen: &mut Pen, sheet: &ScreenSheet, (pw, ph): (f32, f32), piece: Piece) {
    let f = pen.faces;
    let (w, h) = (piece.x[1] - piece.x[0], piece.y[1] - piece.y[0]);
    let (rw, rh) = room((pw, ph));
    let left = MARGIN + BAND + (rw - w).max(0.0) / 2.0;
    let top = MARGIN + HEADING + BAND + (rh - h).max(0.0) / 2.0;
    let at = |x: f32, y: f32| [left + x - piece.x[0], top + piece.y[1] - y];
    let eps = 0.01;
    // Which edges are the screen's own (with their band): the others butt
    // against the next page.
    let outer_left = piece.x[0] < eps;
    let outer_right = piece.x[1] > sheet.width_mm - eps;
    let outer_front = piece.y[0] < eps;
    let outer_back = piece.y[1] > sheet.depth_mm - eps;

    // The zones already marked.
    let (hw, hd) = (sheet.width_mm / 2.0, sheet.depth_mm / 2.0);
    for zone in &sheet.zones {
        let x0 = (zone.min[0] as f32 + hw).max(piece.x[0]);
        let x1 = (zone.max[0] as f32 + hw).min(piece.x[1]);
        let y0 = (zone.min[1] as f32 + hd).max(piece.y[0]);
        let y1 = (zone.max[1] as f32 + hd).min(piece.y[1]);
        if x1 - x0 > eps && y1 - y0 > eps {
            let [px, py] = at(x0, y1);
            pen.rect(
                px,
                py,
                x1 - x0,
                y1 - y0,
                Some(fill(DEAD)),
                Some(stroke(DEAD_EDGE, 0.4 * PT)),
            );
        }
    }

    // The grid: every 5 mm, stronger every 10 and 50, from the front left.
    let line_of = |mm: f32| {
        let k = (mm / CELL_MM as f32).round() as i32;
        if k % 10 == 0 {
            stroke(GRID_MAJOR_DARK, 0.45 * PT)
        } else if k % 2 == 0 {
            stroke(TEN_LINE, 0.3 * PT)
        } else {
            stroke(CELL_LINE, 0.22 * PT)
        }
    };
    let cell = CELL_MM as f32;
    let mut mm = (piece.x[0] / cell).floor() * cell + cell;
    while mm < piece.x[1] - eps {
        pen.line(at(mm, piece.y[0]), at(mm, piece.y[1]), line_of(mm));
        mm += cell;
    }
    let mut mm = (piece.y[0] / cell).floor() * cell + cell;
    while mm < piece.y[1] - eps {
        pen.line(at(piece.x[0], mm), at(piece.x[1], mm), line_of(mm));
        mm += cell;
    }

    // The screen's edge: thick where it is its own, thin where the next
    // page goes on.
    let edge = |pen: &mut Pen, a: [f32; 2], b: [f32; 2], own: bool| {
        pen.line(a, b, stroke(INK, if own { 0.9 } else { 0.5 } * PT));
    };
    let (fl, fr) = (at(piece.x[0], piece.y[0]), at(piece.x[1], piece.y[0]));
    let (bl, br) = (at(piece.x[0], piece.y[1]), at(piece.x[1], piece.y[1]));
    edge(pen, fl, fr, outer_front);
    edge(pen, bl, br, outer_back);
    edge(pen, fl, bl, outer_left);
    edge(pen, fr, br, outer_right);

    // The line to cut along: a band out where the edge is the screen's.
    let out = |own: bool| if own { BAND } else { 0.0 };
    let cut = [
        bl[0] - out(outer_left),
        bl[1] - out(outer_back),
        br[0] + out(outer_right),
        fl[1] + out(outer_front),
    ];
    let mut dashed = stroke(INK_SOFT, 0.4 * PT);
    dashed.dash = Some(StrokeDash {
        array: vec![1.6, 1.1],
        offset: 0.0,
    });
    dashed.line_cap = LineCap::Butt;
    pen.rect(
        cut[0],
        cut[1],
        cut[2] - cut[0],
        cut[3] - cut[1],
        None,
        Some(dashed),
    );

    // The cells' names: columns in the bands at the front and the back,
    // rows at the sides; where the next page goes on, inside the edge cells.
    let size = 5.0 * PT;
    let columns = (piece.x[0] / cell).round() as u32..(piece.x[1] / cell - eps).ceil() as u32;
    let rows = (piece.y[0] / cell).round() as u32..(piece.y[1] / cell - eps).ceil() as u32;
    for col in columns.clone() {
        let x0 = col as f32 * cell;
        let x1 = (x0 + cell).min(sheet.width_mm);
        // A sliver of a cell at the screen's edge has no room for a name.
        if x1 - x0 < cell / 2.0 {
            continue;
        }
        let middle = (x0 + x1) / 2.0;
        let name = column_letters(col);
        let front = at(middle, piece.y[0]);
        let back = at(middle, piece.y[1]);
        if outer_front {
            pen.text_centre(&f.medium, size, front[0], front[1] + 2.4, &name, INK_SOFT);
        } else {
            pen.text_centre(&f.regular, 4.0 * PT, front[0], front[1] - 0.8, &name, MUTED);
        }
        if outer_back {
            pen.text_centre(&f.medium, size, back[0], back[1] - 1.0, &name, INK_SOFT);
        } else {
            pen.text_centre(&f.regular, 4.0 * PT, back[0], back[1] + 2.0, &name, MUTED);
        }
    }
    for row in rows {
        let y0 = row as f32 * cell;
        let y1 = (y0 + cell).min(sheet.depth_mm);
        if y1 - y0 < cell / 2.0 {
            continue;
        }
        let middle = (y0 + y1) / 2.0;
        let name = (row + 1).to_string();
        let l = at(piece.x[0], middle);
        let r = at(piece.x[1], middle);
        if outer_left {
            pen.text_right(&f.medium, size, l[0] - 0.8, l[1] + 0.7, &name, INK_SOFT);
        } else {
            pen.text(&f.regular, 4.0 * PT, l[0] + 0.5, l[1] + 0.6, &name, MUTED);
        }
        if outer_right {
            pen.text(&f.medium, size, r[0] + 0.8, r[1] + 0.7, &name, INK_SOFT);
        } else {
            pen.text_right(&f.regular, 4.0 * PT, r[0] - 0.5, r[1] + 0.6, &name, MUTED);
        }
    }

    // The front, as the plate says it in the app, with arrows to it.
    if outer_front {
        let tab_w = spaced_width(&f.bold, 5.0 * PT, &sheet.texts.front, 0.35) + 4.0;
        let middle = (fl[0] + fr[0]) / 2.0;
        let tab_l = middle - tab_w / 2.0;
        let tab_t = fl[1] + 3.4;
        pen.rounded([tab_l, tab_t, tab_w, 3.0], 0.5, Some(fill(INK)), None);
        pen.spaced(
            &f.bold,
            5.0 * PT,
            [tab_l + 2.0, tab_t + 2.15],
            &sheet.texts.front,
            0.35,
            PAPER,
        );
        for dx in [-tab_w / 2.0 - 4.0, tab_w / 2.0 + 4.0] {
            let x = middle + dx;
            let y = tab_t + 1.5;
            pen.path(
                |pb| {
                    pb.move_to(x - 1.3, y - 0.9);
                    pb.line_to(x + 1.3, y - 0.9);
                    pb.line_to(x, y + 0.9);
                    pb.close();
                },
                Some(fill(INK)),
                None,
            );
        }
    }
}

/// The grid's line every 50 mm, darker than the plate checklist's: the
/// sheet is read against a light.
const GRID_MAJOR_DARK: Rgb = (0x9a, 0x92, 0x88);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_that_fits_takes_one_landscape_page() {
        // The M7 Pro's screen.
        let layout = layout(223.0, 126.0);
        assert_eq!(layout.page, (297.0, 210.0));
        assert_eq!(layout.pieces.len(), 1);
        assert_eq!(layout.pieces[0].x, [0.0, 223.0]);
    }

    #[test]
    fn a_wider_screen_splits_along_grid_lines() {
        let layout = layout(298.0, 164.0);
        assert!(layout.pieces.len() >= 2);
        let (rw, rh) = room(layout.page);
        for piece in &layout.pieces {
            assert!(piece.x[1] - piece.x[0] <= rw + 0.01);
            assert!(piece.y[1] - piece.y[0] <= rh + 0.01);
            for edge in [piece.x[0], piece.y[0]] {
                assert!((edge / 10.0).fract().abs() < 1e-4, "{edge}");
            }
        }
        // The pieces cover the screen once.
        let area: f32 = layout
            .pieces
            .iter()
            .map(|p| (p.x[1] - p.x[0]) * (p.y[1] - p.y[0]))
            .sum();
        assert!((area - 298.0 * 164.0).abs() < 0.1);
    }

    #[test]
    fn the_sheet_is_written() {
        let sheet = ScreenSheet {
            printer: "Anycubic Photon Mono M7 Pro".into(),
            version: "test".into(),
            width_mm: 223.0,
            depth_mm: 126.0,
            zones: vec![DeadZone::between([-20.0, -10.0], [-10.0, 0.0])],
            ..Default::default()
        };
        let bytes = write(&sheet).expect("a PDF");
        assert!(bytes.starts_with(b"%PDF"));
        if let Ok(path) = std::env::var("BREADSLICER_SCREEN_SHEET") {
            std::fs::write(path, &bytes).unwrap();
        }
        let wide = ScreenSheet {
            width_mm: 298.0,
            depth_mm: 164.0,
            ..sheet
        };
        assert!(write(&wide).expect("a PDF").starts_with(b"%PDF"));
    }
}
