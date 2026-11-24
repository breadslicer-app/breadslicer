/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The plate checklist drawn as a PDF (#740): A4 pages of four plates, each
//! a plan seen from above with every part numbered, and beside it the list
//! to tick.
//!
//! Everything is laid out in millimetres, y down from the page's top left.

use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, LineCap, LineJoin, Stroke, StrokeDash};
use krilla::surface::Surface;
use krilla::text::{Font, Tag, TextDirection};
use krilla::{Data, Document};

use super::outline::{Outline, Shape};
use super::{Sheet, SheetPart, SheetPlate};

/// The app's typeface: Noto Sans, variable in weight and width.
static NOTO_SANS: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Variable.ttf");

const PAGE_W: f32 = 210.0;
const PAGE_H: f32 = 297.0;
const MARGIN: f32 = 13.0;
const PLATES_PER_PAGE: usize = 4;
/// Where the plates start, under the heading.
const BODY_TOP: f32 = 37.0;
/// Where they end, over the footer.
const BODY_BOTTOM: f32 = PAGE_H - 16.0;
/// The plan's column: its left edge and the most it takes.
const PLAN_LEFT: f32 = MARGIN + 4.0;
const PLAN_MAX_W: f32 = 112.0;
/// The room between the plan and the list.
const LIST_GAP: f32 = 13.0;
/// The list's column: its right edge, and the least it starts at.
const LIST_RIGHT: f32 = PAGE_W - MARGIN;
const LIST_MIN_LEFT: f32 = 92.0;
/// The plan's room under a plate's heading, and over its rulers.
const PLAN_TOP: f32 = 9.0;
const PLAN_UNDER: f32 = 6.0;

/// A point, mm.
const PT: f32 = 25.4 / 72.0;

type Rgb = (u8, u8, u8);
const INK: Rgb = (0x1c, 0x1f, 0x23);
const INK_SOFT: Rgb = (0x4b, 0x50, 0x57);
const MUTED: Rgb = (0x8b, 0x90, 0x96);
const HAIR: Rgb = (0xd8, 0xd4, 0xce);
const PAPER: Rgb = (0xff, 0xff, 0xff);
const PLATE: Rgb = (0xf6, 0xf3, 0xef);
const GRID: Rgb = (0xe8, 0xe3, 0xdc);
const GRID_MAJOR: Rgb = (0xd6, 0xcf, 0xc5);
const AMBER: Rgb = (0xe0, 0x8a, 0x3c);
const SAND: Rgb = (0xec, 0xdc, 0xc5);
const SAND_EDGE: Rgb = (0xb0, 0x95, 0x72);
const SHARED: Rgb = (0xf1, 0xea, 0xe0);

/// A weight and width of the typeface, to draw and to measure with.
struct Face {
    font: Font,
    coords: Vec<i16>,
}

impl Face {
    fn new(weight: f32, width: f32) -> Self {
        let font = Font::new_variable(
            Data::from(NOTO_SANS),
            0,
            &[(Tag::new(b"wght"), weight), (Tag::new(b"wdth"), width)],
        )
        .expect("the app's typeface reads");
        let coords = swash::FontRef::from_index(NOTO_SANS, 0)
            .map(|f| {
                f.variations()
                    .normalized_coords([("wght", weight), ("wdth", width)])
                    .collect()
            })
            .unwrap_or_default();
        Self { font, coords }
    }

    /// How wide `text` runs at `size` (mm).
    fn width(&self, text: &str, size: f32) -> f32 {
        let Some(font) = swash::FontRef::from_index(NOTO_SANS, 0) else {
            return text.chars().count() as f32 * size * 0.55;
        };
        let metrics = font.glyph_metrics(&self.coords).scale(size);
        let map = font.charmap();
        text.chars()
            .map(|c| metrics.advance_width(map.map(c)))
            .sum()
    }
}

struct Faces {
    regular: Face,
    medium: Face,
    semibold: Face,
    bold: Face,
    /// Heavy and narrow, for the title and the plates' numbers.
    display: Face,
}

impl Faces {
    fn new() -> Self {
        Self {
            regular: Face::new(400.0, 100.0),
            medium: Face::new(500.0, 100.0),
            semibold: Face::new(600.0, 100.0),
            bold: Face::new(700.0, 100.0),
            display: Face::new(850.0, 75.0),
        }
    }
}

fn color((r, g, b): Rgb) -> krilla::paint::Paint {
    rgb::Color::new(r, g, b).into()
}

fn fill(c: Rgb) -> Fill {
    Fill {
        paint: color(c),
        opacity: NormalizedF32::ONE,
        rule: FillRule::NonZero,
    }
}

fn stroke(c: Rgb, width: f32) -> Stroke {
    Stroke {
        paint: color(c),
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

/// A page being drawn: its surface, in millimetres.
struct Pen<'a, 'b> {
    s: &'a mut Surface<'b>,
    faces: &'a Faces,
}

impl Pen<'_, '_> {
    fn path(
        &mut self,
        build: impl FnOnce(&mut PathBuilder),
        fill_with: Option<Fill>,
        stroke_with: Option<Stroke>,
    ) {
        let mut pb = PathBuilder::new();
        build(&mut pb);
        let Some(path) = pb.finish() else {
            return;
        };
        self.s.set_fill(fill_with);
        self.s.set_stroke(stroke_with);
        self.s.draw_path(&path);
        self.s.set_stroke(None);
    }

    fn line(&mut self, a: [f32; 2], b: [f32; 2], with: Stroke) {
        self.path(
            |pb| {
                pb.move_to(a[0], a[1]);
                pb.line_to(b[0], b[1]);
            },
            None,
            Some(with),
        );
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, f: Option<Fill>, st: Option<Stroke>) {
        self.path(|pb| rounded(pb, x, y, w, h, 0.0), f, st);
    }

    fn rounded(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        f: Option<Fill>,
        st: Option<Stroke>,
    ) {
        self.path(|pb| rounded(pb, x, y, w, h, r), f, st);
    }

    fn circle(&mut self, c: [f32; 2], r: f32, f: Option<Fill>, st: Option<Stroke>) {
        self.path(|pb| circle(pb, c, r), f, st);
    }

    /// `text` with its baseline's start at `x, y`.
    fn text(&mut self, face: &Face, size: f32, x: f32, y: f32, text: &str, c: Rgb) {
        if text.is_empty() {
            return;
        }
        self.s.set_fill(Some(fill(c)));
        self.s.draw_text(
            Point::from_xy(x, y),
            face.font.clone(),
            size,
            text,
            false,
            TextDirection::Auto,
        );
    }

    fn text_right(&mut self, face: &Face, size: f32, right: f32, y: f32, text: &str, c: Rgb) {
        let w = face.width(text, size);
        self.text(face, size, right - w, y, text, c);
    }

    fn text_centre(&mut self, face: &Face, size: f32, centre: f32, y: f32, text: &str, c: Rgb) {
        let w = face.width(text, size);
        self.text(face, size, centre - w / 2.0, y, text, c);
    }

    /// `text` with `tracking` mm between its letters; returns how wide.
    fn spaced(
        &mut self,
        face: &Face,
        size: f32,
        x: f32,
        y: f32,
        text: &str,
        tracking: f32,
        c: Rgb,
    ) -> f32 {
        let mut at = x;
        for ch in text.chars() {
            let s = ch.to_string();
            self.text(face, size, at, y, &s, c);
            at += face.width(&s, size) + tracking;
        }
        at - x - tracking
    }
}

fn spaced_width(face: &Face, size: f32, text: &str, tracking: f32) -> f32 {
    let n = text.chars().count();
    face.width(text, size) + tracking * n.saturating_sub(1) as f32
}

/// `text` cut with an ellipsis to fit `max` mm at `size`.
fn fit(face: &Face, size: f32, text: &str, max: f32) -> String {
    if face.width(text, size) <= max {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let cut: String = chars.iter().collect::<String>().trim_end().to_owned() + "…";
        if face.width(&cut, size) <= max {
            return cut;
        }
    }
    "…".to_owned()
}

fn rounded(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        pb.move_to(x, y);
        pb.line_to(x + w, y);
        pb.line_to(x + w, y + h);
        pb.line_to(x, y + h);
        pb.close();
        return;
    }
    let k = r * 0.552_284_8;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
}

fn circle(pb: &mut PathBuilder, c: [f32; 2], r: f32) {
    rounded(pb, c[0] - r, c[1] - r, 2.0 * r, 2.0 * r, r);
}

/// The PDF of `sheet`.
pub(crate) fn write(sheet: &Sheet) -> Result<Vec<u8>, String> {
    let faces = Faces::new();
    let mut document = Document::new();
    document.set_metadata(
        krilla::metadata::Metadata::new()
            .title(format!("{} · {}", sheet.texts.title, sheet.project))
            .creator(format!("Breadslicer {}", sheet.version))
            .producer(format!("Breadslicer {}", sheet.version)),
    );
    let plates: Vec<&SheetPlate> = sheet.plates.iter().collect();
    let pages = plates.len().div_ceil(PLATES_PER_PAGE).max(1);
    let size = PageSettings::from_wh(PAGE_W / PT, PAGE_H / PT).ok_or("page size")?;
    for page_index in 0..pages {
        let mut page = document.start_page_with(size.clone());
        let mut surface = page.surface();
        surface.push_transform(&Transform::from_scale(1.0 / PT, 1.0 / PT));
        {
            let mut pen = Pen {
                s: &mut surface,
                faces: &faces,
            };
            pen.rect(0.0, 0.0, PAGE_W, PAGE_H, Some(fill(PAPER)), None);
            heading(&mut pen, sheet, page_index, pages);
            let row_h = (BODY_BOTTOM - BODY_TOP) / PLATES_PER_PAGE as f32;
            let on_page = plates
                .iter()
                .skip(page_index * PLATES_PER_PAGE)
                .take(PLATES_PER_PAGE);
            for (row, plate) in on_page.enumerate() {
                let top = BODY_TOP + row as f32 * row_h;
                if row > 0 {
                    let mut dashed = stroke(HAIR, 0.5 * PT);
                    dashed.dash = Some(StrokeDash {
                        array: vec![1.2, 1.0],
                        offset: 0.0,
                    });
                    dashed.line_cap = LineCap::Butt;
                    pen.line([MARGIN, top - 1.5], [PAGE_W - MARGIN, top - 1.5], dashed);
                }
                plate_row(&mut pen, sheet, plate, top, row_h - 3.0);
            }
            footer(&mut pen, sheet);
        }
        surface.pop();
        surface.finish();
        page.finish();
    }
    document.finish().map_err(|err| format!("{err:?}"))
}

fn heading(pen: &mut Pen, sheet: &Sheet, page: usize, pages: usize) {
    let f = pen.faces;
    let top = MARGIN;
    // The brand, tracked out, over the title.
    pen.circle([MARGIN + 1.1, top + 2.6], 1.1, Some(fill(AMBER)), None);
    pen.spaced(
        &f.bold,
        6.2 * PT,
        MARGIN + 3.6,
        top + 3.6,
        "BREADSLICER",
        0.62,
        INK_SOFT,
    );
    pen.text(
        &f.display,
        25.0 * PT,
        MARGIN - 0.4,
        top + 14.2,
        &sheet.texts.title,
        INK,
    );
    // The project, and what it prints with.
    let right = PAGE_W - MARGIN;
    let project = fit(&f.bold, 11.5 * PT, &sheet.project, 95.0);
    pen.text_right(&f.bold, 11.5 * PT, right, top + 5.0, &project, INK);
    let with = [sheet.printer.as_str(), sheet.resin.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("  ·  ");
    let with = fit(&f.medium, 7.6 * PT, &with, 110.0);
    pen.text_right(&f.medium, 7.6 * PT, right, top + 9.6, &with, INK_SOFT);
    let when = format!(
        "{}  ·  {}",
        sheet.date,
        sheet
            .texts
            .page
            .replace("{page}", &(page + 1).to_string())
            .replace("{pages}", &pages.to_string())
    );
    pen.text_right(&f.regular, 7.0 * PT, right, top + 13.6, &when, MUTED);
    // A rule, with a short amber bar on its left.
    let rule_y = top + 18.0;
    pen.line([MARGIN, rule_y], [right, rule_y], stroke(INK, 0.7 * PT));
    pen.rect(MARGIN, rule_y - 0.75, 22.0, 1.5, Some(fill(AMBER)), None);
}

fn footer(pen: &mut Pen, sheet: &Sheet) {
    let f = pen.faces;
    let y = PAGE_H - MARGIN + 3.6;
    pen.line(
        [MARGIN, PAGE_H - MARGIN],
        [PAGE_W - MARGIN, PAGE_H - MARGIN],
        stroke(HAIR, 0.5 * PT),
    );
    let made = format!("Breadslicer {}", sheet.version);
    pen.text(&f.semibold, 6.2 * PT, MARGIN, y, &made, MUTED);
    // How to read it: a tick box, and the hint.
    let hint = &sheet.texts.hint;
    let w = f.regular.width(hint, 6.2 * PT) + 3.4;
    let x = (PAGE_W - w) / 2.0;
    pen.rounded(
        x,
        y - 2.0,
        2.2,
        2.2,
        0.35,
        None,
        Some(stroke(MUTED, 0.45 * PT)),
    );
    tick(pen, x, y - 2.0, 2.2, MUTED);
    pen.text(&f.regular, 6.2 * PT, x + 3.4, y, hint, MUTED);
    pen.text_right(
        &f.semibold,
        6.2 * PT,
        PAGE_W - MARGIN,
        y,
        "breadslicer.app",
        MUTED,
    );
}

/// A tick drawn in a box at `x, y`, `side` wide.
fn tick(pen: &mut Pen, x: f32, y: f32, side: f32, c: Rgb) {
    pen.path(
        |pb| {
            pb.move_to(x + side * 0.22, y + side * 0.52);
            pb.line_to(x + side * 0.42, y + side * 0.74);
            pb.line_to(x + side * 0.8, y + side * 0.26);
        },
        None,
        Some(stroke(c, 0.55 * PT)),
    );
}

fn plate_row(pen: &mut Pen, sheet: &Sheet, plate: &SheetPlate, top: f32, height: f32) {
    let f = pen.faces;
    // The plate's number, its name and its size.
    let number = format!("{:02}", plate.number);
    let base = top + 5.0;
    pen.text(&f.display, 15.0 * PT, MARGIN, base, &number, AMBER);
    let after = MARGIN + f.display.width(&number, 15.0 * PT) + 2.4;
    let name = fit(&f.bold, 10.0 * PT, &plate.name, 70.0);
    pen.text(&f.bold, 10.0 * PT, after, base, &name, INK);
    let dims = format!(
        "{} × {} mm",
        trim_mm(plate.width_mm),
        trim_mm(plate.depth_mm)
    );
    let dims_x = after + f.bold.width(&name, 10.0 * PT) + 2.6;
    pen.text(&f.regular, 7.0 * PT, dims_x, base, &dims, MUTED);
    let dims_end = dims_x + f.regular.width(&dims, 7.0 * PT);
    if let Some(info) = &plate.info {
        let info = fit(&f.medium, 7.0 * PT, info, LIST_RIGHT - dims_end - 8.0);
        pen.text_right(&f.medium, 7.0 * PT, LIST_RIGHT, base, &info, INK_SOFT);
    }
    // The plan, as large as its room lets it, and the list beside it.
    let (plan_top, plan_h) = (top + PLAN_TOP, height - PLAN_TOP - PLAN_UNDER);
    let scale = plan_scale(plate, plan_h);
    let (w, h) = (plate.width_mm * scale, plate.depth_mm * scale);
    plan(
        pen,
        sheet,
        plate,
        PLAN_LEFT,
        plan_top + (plan_h - h) / 2.0,
        scale,
    );
    let list_left = (PLAN_LEFT + w + LIST_GAP).max(LIST_MIN_LEFT);
    list(pen, sheet, plate, list_left, top + PLAN_TOP, top + height);
}

/// Paper mm a plate mm, for a plan `room` mm high.
fn plan_scale(plate: &SheetPlate, room: f32) -> f32 {
    (PLAN_MAX_W / plate.width_mm).min(room / plate.depth_mm)
}

fn trim_mm(mm: f32) -> String {
    let tenths = (mm * 10.0).round();
    if tenths % 10.0 == 0.0 {
        format!("{}", tenths / 10.0)
    } else {
        format!("{:.1}", tenths / 10.0)
    }
}

/// The plate seen from above, its front down, at `scale` (paper mm a plate mm).
fn plan(pen: &mut Pen, sheet: &Sheet, plate: &SheetPlate, left: f32, top: f32, scale: f32) {
    let f = pen.faces;
    let (w, h) = (plate.width_mm * scale, plate.depth_mm * scale);
    let at = |p: [f32; 2]| {
        [
            left + (p[0] + plate.width_mm / 2.0) * scale,
            top + (plate.depth_mm / 2.0 - p[1]) * scale,
        ]
    };
    pen.rect(left, top, w, h, Some(fill(PLATE)), None);
    // The grid, every 10 mm from the front left corner, stronger every 50.
    let mut mm = 10.0;
    while mm < plate.width_mm - 0.01 {
        let x = left + mm * scale;
        let major = (mm as i32) % 50 == 0;
        let c = if major { GRID_MAJOR } else { GRID };
        pen.line(
            [x, top],
            [x, top + h],
            stroke(c, if major { 0.45 } else { 0.3 } * PT),
        );
        mm += 10.0;
    }
    let mut mm = 10.0;
    while mm < plate.depth_mm - 0.01 {
        let y = top + h - mm * scale;
        let major = (mm as i32) % 50 == 0;
        let c = if major { GRID_MAJOR } else { GRID };
        pen.line(
            [left, y],
            [left + w, y],
            stroke(c, if major { 0.45 } else { 0.3 } * PT),
        );
        mm += 10.0;
    }
    // What stands on it: rafts for all, footprints, outlines, numbers.
    let shape_path = |pb: &mut PathBuilder, shape: &Shape| {
        for outline in &shape.outlines {
            outline_path(pb, outline, &at);
        }
    };
    for shared in &plate.shared {
        let mut edge = stroke(SAND_EDGE, 0.4 * PT);
        edge.dash = Some(StrokeDash {
            array: vec![0.9, 0.6],
            offset: 0.0,
        });
        pen.path(|pb| shape_path(pb, shared), Some(fill(SHARED)), Some(edge));
    }
    for part in &plate.parts {
        pen.path(
            |pb| shape_path(pb, &part.base),
            Some(fill(SAND)),
            Some(stroke(SAND_EDGE, 0.4 * PT)),
        );
    }
    for part in &plate.parts {
        pen.path(
            |pb| shape_path(pb, &part.top),
            None,
            Some(stroke(INK, 0.5 * PT)),
        );
    }
    if plate.parts.is_empty() {
        pen.text_centre(
            &f.medium,
            7.5 * PT,
            left + w / 2.0,
            top + h / 2.0 + 1.0,
            &sheet.texts.empty_plate,
            MUTED,
        );
    }
    for part in &plate.parts {
        if let Some(p) = label_point(part) {
            badge(pen, at(p), part.index);
        }
    }
    // The plate's edge, its rulers and corner marks.
    pen.rect(left, top, w, h, None, Some(stroke(INK, 0.75 * PT)));
    let tab_w = spaced_width(&f.bold, 5.0 * PT, &sheet.texts.front, 0.35) + 4.0;
    let tab_l = left + (w - tab_w) / 2.0;
    let mut mm = 0.0;
    while mm <= plate.width_mm + 0.01 {
        let x = left + mm * scale;
        let major = (mm as i32) % 50 == 0;
        let len = if major { 1.7 } else { 0.9 };
        pen.line([x, top + h], [x, top + h + len], stroke(INK_SOFT, 0.4 * PT));
        if major && (x < tab_l - 3.0 || x > tab_l + tab_w + 3.0) {
            pen.text_centre(
                &f.regular,
                4.6 * PT,
                x,
                top + h + 3.9,
                &format!("{mm}"),
                MUTED,
            );
        }
        mm += 10.0;
    }
    let mut mm = 0.0;
    while mm <= plate.depth_mm + 0.01 {
        let y = top + h - mm * scale;
        let major = (mm as i32) % 50 == 0;
        let len = if major { 1.7 } else { 0.9 };
        pen.line([left - len, y], [left, y], stroke(INK_SOFT, 0.4 * PT));
        if major && mm > 0.0 {
            pen.text_right(
                &f.regular,
                4.6 * PT,
                left - 2.2,
                y + 0.6,
                &format!("{mm}"),
                MUTED,
            );
        }
        mm += 10.0;
    }
    // The front, as the plate says it in the app.
    pen.rounded(tab_l, top + h + 0.9, tab_w, 3.0, 0.5, Some(fill(INK)), None);
    pen.spaced(
        &f.bold,
        5.0 * PT,
        tab_l + 2.0,
        top + h + 3.05,
        &sheet.texts.front,
        0.35,
        PAPER,
    );
    for (cx, cy, dx, dy) in [
        (left + w, top, 1.0, -1.0),
        (left, top, -1.0, -1.0),
        (left + w, top + h, 1.0, 1.0),
    ] {
        let (o, l) = (1.2, 2.6);
        let (x, y) = (cx + dx * o, cy + dy * o);
        pen.path(
            |pb| {
                pb.move_to(x - dx * l, y);
                pb.line_to(x, y);
                pb.line_to(x, y - dy * l);
            },
            None,
            Some(stroke(INK, 0.5 * PT)),
        );
    }
}

fn outline_path(pb: &mut PathBuilder, outline: &Outline, at: &impl Fn([f32; 2]) -> [f32; 2]) {
    let mut points = outline.iter().map(|&p| at(p));
    let Some(first) = points.next() else {
        return;
    };
    pb.move_to(first[0], first[1]);
    for p in points {
        pb.line_to(p[0], p[1]);
    }
    pb.close();
}

/// Where a part's number goes: inside what it leaves on the plate, or
/// inside its outline from above when that leaves little.
fn label_point(part: &SheetPart) -> Option<[f32; 2]> {
    part.base.label_at.or(part.top.label_at)
}

fn badge(pen: &mut Pen, c: [f32; 2], index: usize) {
    let f = pen.faces;
    let text = index.to_string();
    let size = if text.len() > 2 { 4.6 } else { 5.6 } * PT;
    let r = if text.len() > 2 { 2.15 } else { 1.85 };
    pen.circle(c, r + 0.35, Some(fill(PAPER)), None);
    pen.circle(c, r, Some(fill(INK)), None);
    pen.text_centre(&f.bold, size, c[0], c[1] + size * 0.36, &text, PAPER);
}

/// The parts of a plate grouped by name, in the order their first number
/// comes, each with its numbers.
fn groups(plate: &SheetPlate) -> Vec<(&str, Vec<usize>)> {
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    let mut parts: Vec<&SheetPart> = plate.parts.iter().collect();
    parts.sort_by_key(|p| p.index);
    for part in parts {
        match groups.iter_mut().find(|(name, _)| *name == part.name) {
            Some((_, numbers)) => numbers.push(part.index),
            None => groups.push((&part.name, vec![part.index])),
        }
    }
    groups
}

/// How a list is laid out: its size, its columns.
#[derive(Clone, Copy)]
struct ListFit {
    k: f32,
    columns: usize,
}

impl ListFit {
    fn name_size(self) -> f32 {
        7.2 * PT * self.k
    }
    fn chip_size(self) -> f32 {
        6.2 * PT * self.k
    }
    fn box_side(self) -> f32 {
        3.0 * self.k
    }
    fn name_line(self) -> f32 {
        3.7 * self.k
    }
    fn chip_line(self) -> f32 {
        4.3 * self.k
    }
    fn group_gap(self) -> f32 {
        1.6 * self.k
    }
}

/// Lays out `groups` in a column `width` wide: for each group, where its
/// name goes and where each chip goes, relative to the group's top; and the
/// group's height.
fn layout_group(
    f: &Faces,
    fit_: ListFit,
    numbers: &[usize],
    width: f32,
) -> (Vec<(f32, f32, usize)>, f32) {
    // A part alone has its box on its name's line.
    if let [n] = numbers {
        return (vec![(0.0, 0.0, *n)], fit_.chip_line());
    }
    let mut chips = Vec::new();
    let (mut x, mut y) = (0.0, fit_.name_line());
    for &n in numbers {
        let w = fit_.box_side()
            + 0.9
            + f.semibold.width(&n.to_string(), fit_.chip_size())
            + 2.2 * fit_.k;
        if x > 0.0 && x + w > width {
            x = 0.0;
            y += fit_.chip_line();
        }
        chips.push((x, y, n));
        x += w;
    }
    (chips, y + fit_.chip_line())
}

fn list_height(f: &Faces, groups: &[(&str, Vec<usize>)], fit_: ListFit, width: f32) -> f32 {
    let col_w = (width - 4.0 * (fit_.columns as f32 - 1.0)) / fit_.columns as f32;
    let heights: Vec<f32> = groups
        .iter()
        .map(|(_, n)| layout_group(f, fit_, n, col_w).1 + fit_.group_gap())
        .collect();
    let total: f32 = heights.iter().sum();
    if fit_.columns == 1 {
        return total;
    }
    // Columns filled in order, each to the height of an even share.
    let target = total / fit_.columns as f32;
    let (mut tallest, mut run, mut col) = (0.0f32, 0.0f32, 1);
    for h in heights {
        if run > 0.0 && run + h > target + 0.01 && col < fit_.columns {
            tallest = tallest.max(run);
            run = 0.0;
            col += 1;
        }
        run += h;
    }
    tallest.max(run)
}

/// The largest size, in as few columns as can be, that fits `groups` in
/// `width` × `room` mm; `None` when even the smallest does not.
fn choose_fit(f: &Faces, groups: &[(&str, Vec<usize>)], width: f32, room: f32) -> Option<ListFit> {
    [1.0, 0.92, 0.85, 0.78]
        .map(|k| ListFit { k, columns: 1 })
        .into_iter()
        .chain([0.85, 0.78, 0.7, 0.62, 0.55].map(|k| ListFit { k, columns: 2 }))
        .chain([0.62, 0.55, 0.5].map(|k| ListFit { k, columns: 3 }))
        .find(|&t| list_height(f, groups, t, width) <= room)
}

fn list(pen: &mut Pen, sheet: &Sheet, plate: &SheetPlate, left: f32, top: f32, bottom: f32) {
    let f = pen.faces;
    let width = LIST_RIGHT - left;
    // Its heading: PARTS, and how many.
    pen.spaced(
        &f.bold,
        5.6 * PT,
        left,
        top + 2.0,
        &sheet.texts.parts,
        0.45,
        MUTED,
    );
    let count = plate.parts.len().to_string();
    pen.text_right(&f.bold, 7.5 * PT, LIST_RIGHT, top + 2.2, &count, INK);
    pen.line(
        [left, top + 3.6],
        [LIST_RIGHT, top + 3.6],
        stroke(HAIR, 0.5 * PT),
    );
    let groups = groups(plate);
    if groups.is_empty() {
        pen.text(&f.regular, 7.0 * PT, left, top + 8.0, "—", MUTED);
        return;
    }
    let room = bottom - (top + 5.6);
    let fit_ = choose_fit(f, &groups, width, room).unwrap_or(ListFit { k: 0.5, columns: 3 });
    let col_w = (width - 4.0 * (fit_.columns as f32 - 1.0)) / fit_.columns as f32;
    let heights: Vec<f32> = groups
        .iter()
        .map(|(_, n)| layout_group(f, fit_, n, col_w).1 + fit_.group_gap())
        .collect();
    let target = heights.iter().sum::<f32>() / fit_.columns as f32;
    let (mut col, mut y) = (0usize, top + 5.6);
    let mut run = 0.0;
    for ((name, numbers), h) in groups.iter().zip(&heights) {
        if run > 0.0 && run + h > target + 0.01 && col + 1 < fit_.columns {
            col += 1;
            y = top + 5.6;
            run = 0.0;
        }
        if y + h - fit_.group_gap() > bottom + 0.5 {
            // Past the room left: say how many more there are.
            let left_out: usize = groups
                .iter()
                .skip_while(|(n, _)| n != name)
                .map(|(_, n)| n.len())
                .sum();
            let more = sheet.texts.more.replace("{n}", &left_out.to_string());
            pen.text(
                &f.semibold,
                fit_.chip_size(),
                left + col as f32 * (col_w + 4.0),
                y + fit_.name_line() - 0.8,
                &more,
                MUTED,
            );
            break;
        }
        let x0 = left + col as f32 * (col_w + 4.0);
        let (chips, _) = layout_group(f, fit_, numbers, col_w);
        let alone = numbers.len() == 1;
        let side = fit_.box_side();
        // A part alone: its name after its box and number.
        let (name_x, base) = if alone {
            let number_w = f.semibold.width(&numbers[0].to_string(), fit_.chip_size());
            let by = y + (fit_.chip_line() - side) / 2.0 - 0.15 * fit_.k;
            (
                x0 + side + 0.9 + number_w + 2.2 * fit_.k,
                by + side * 0.5 + fit_.chip_size() * 0.36,
            )
        } else {
            (x0, y + fit_.name_line() - 0.95 * fit_.k)
        };
        let times = (numbers.len() > 1).then(|| format!("× {}", numbers.len()));
        let times_w = times
            .as_deref()
            .map_or(0.0, |t| f.medium.width(t, fit_.chip_size()) + 1.5);
        let shown = fit(
            &f.semibold,
            fit_.name_size(),
            name,
            x0 + col_w - name_x - times_w,
        );
        pen.text(&f.semibold, fit_.name_size(), name_x, base, &shown, INK);
        if let Some(t) = times {
            pen.text_right(&f.medium, fit_.chip_size(), x0 + col_w, base, &t, MUTED);
        }
        for (cx, cy, n) in chips {
            let bx = x0 + cx;
            let by = y + cy + (fit_.chip_line() - side) / 2.0 - 0.15 * fit_.k;
            pen.rounded(
                bx,
                by,
                side,
                side,
                0.4 * fit_.k,
                Some(fill(PAPER)),
                Some(stroke(INK_SOFT, 0.5 * PT)),
            );
            pen.text(
                &f.semibold,
                fit_.chip_size(),
                bx + side + 0.9,
                by + side * 0.5 + fit_.chip_size() * 0.36,
                &n.to_string(),
                INK_SOFT,
            );
        }
        y += h;
        run += h;
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_sheet;
    use super::*;

    /// The sheet comes out as a PDF with a page per four plates.
    #[test]
    fn four_plates_take_a_page() {
        let pdf = write(&test_sheet(5, 7)).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&pdf);
        assert_eq!(text.matches("/Type/Page/").count(), 2, "two pages");
    }

    /// A long list shrinks and splits into columns rather than spilling.
    #[test]
    fn a_long_list_fits_its_room() {
        let faces = Faces::new();
        let mut sheet = test_sheet(1, 30);
        for (i, p) in sheet.plates[0].parts.iter_mut().enumerate() {
            p.name = format!("A miniature with a long name {i}");
        }
        let g = groups(&sheet.plates[0]);
        let height = (BODY_BOTTOM - BODY_TOP) / PLATES_PER_PAGE as f32 - 3.0;
        let scale = plan_scale(&sheet.plates[0], height - PLAN_TOP - PLAN_UNDER);
        let left = (PLAN_LEFT + sheet.plates[0].width_mm * scale + LIST_GAP).max(LIST_MIN_LEFT);
        let room = height - PLAN_TOP - 5.6;
        assert!(choose_fit(&faces, &g, LIST_RIGHT - left, room).is_some());
        write(&sheet).unwrap();
    }

    #[test]
    fn text_is_cut_to_fit() {
        let faces = Faces::new();
        let long = "Aarakocra Cockatiel Druid of the Northern Woods";
        let cut = fit(&faces.semibold, 7.0 * PT, long, 30.0);
        assert!(cut.ends_with('…'));
        assert!(faces.semibold.width(&cut, 7.0 * PT) <= 30.0);
        assert_eq!(fit(&faces.semibold, 7.0 * PT, "Boar", 30.0), "Boar");
    }
}
