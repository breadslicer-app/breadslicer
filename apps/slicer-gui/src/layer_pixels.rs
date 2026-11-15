/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The layer pane's real printer pixels (#429).
//!
//! The pane's own image (the cap) is a preview of at most 1024 texels across
//! the plate. Over it, this module lays the part of the layer on show at the
//! printer's own resolution: the pixels the printer file holds for that layer,
//! from the very rasterisation the export encodes
//! ([`sc_sla_formats::layer_images::layer_pixels`]), with the greys of its
//! anti-aliasing. The layer is rasterised once, on a worker thread; panning
//! and zooming only cut the part on show out of it.
//!
//! Zoomed out, a texel of the overlay is the mean of the printer pixels it
//! covers; zoomed in, each printer pixel is drawn as a block of texels, in its
//! real shape on panels whose pixels are not square, with a grid between them
//! once a pixel is [`GRID_FROM_PX`] screen pixels across. Under the cursor the
//! pane reads out the pixel's place and grey.
//!
//! The overlay is laid out as the plate is seen from above, as the cap: its
//! columns run from the plate's minimum X, its rows from the plate's back.
//! The file's mirror and panel turn only change where the same pixels sit in
//! the file.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use sc_print_glue::PlateContours;
use sc_print_primitives::{AaLevel, PrinterId};
use sc_print_profiles::PrinterProfile;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::{LayerRaster, RleRun};
use sc_sla_formats::layer_images::layer_pixels;
use sc_sla_formats::{PrinterFileFormat, RasterSetup};
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, SharedString};

use crate::MainWindow;
use crate::i18n::ctr;
use crate::scene_document::SceneDocument;
use crate::slice_panel::SlicePanelBridge;
use crate::viewport::ViewportController;

/// A printer pixel this many screen pixels across, or more, is drawn with a
/// grid line along its far edges.
pub(crate) const GRID_FROM_PX: u32 = 6;

/// The most screen pixels a printer pixel is drawn across. The pane's zoom
/// reaches as far as a pixel 24 screen pixels across; the rest is margin.
const MAX_BLOCK_PX: u32 = 48;

/// The overlay never holds more texels than this: a few times a large pane.
const MAX_TILE_TEXELS: u64 = 4_000_000;

/// While the pane's view moves, the overlay is made again at most this
/// often, and only once the view leaves the part it was made for.
const THROTTLE: std::time::Duration = std::time::Duration::from_millis(50);

/// Once the view keeps still this long, the overlay is made with its margin.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// The overlay reaches this share of the view's width and height past each
/// edge, so a pan shows the printer's pixels at once.
const MARGIN: f32 = 0.5;

/// The pane's background: the overlay is drawn opaque over it, so the
/// coarse image underneath never shows through a dark pixel.
const BACKGROUND_RGB: [u8; 3] = [0x0e, 0x0f, 0x10];

/// Covered pixels' colour: the cap's, so the overlay and the cap read alike.
const FILL_RGB: [u8; 3] = [191, 199, 217];

/// A layer at the printer's resolution, as the printer file holds it.
#[derive(Debug)]
pub(crate) struct PixelLayer {
    /// Printer pixels across the plate's X, and along its Y.
    width: u32,
    height: u32,
    runs: Vec<RleRun>,
    /// Where each raster row starts: its run and the pixels of that run
    /// before the row.
    row_starts: Vec<(u32, u32)>,
    /// The raster's column 0 is the plate's maximum X edge.
    columns_from_max_x: bool,
    /// The raster's row 0 is the plate's maximum Y (back) edge.
    rows_from_max_y: bool,
    z_mm: f64,
    /// The raster it was made with.
    raster: RasterSetup,
}

impl PixelLayer {
    /// `raster`, rasterised with `setup`'s projection
    /// ([`sc_sla_formats::layer_images::plate_pixel_map`]).
    pub(crate) fn new(raster: &LayerRaster, setup: &RasterSetup) -> Self {
        let width = raster.width_px();
        let height = raster.height_px();
        let runs = raster.runs().to_vec();
        let mut row_starts = Vec::with_capacity(height as usize);
        let mut run = 0usize;
        // Pixels of the frame before run `run`.
        let mut before = 0u64;
        for row in 0..u64::from(height) {
            let start = row * u64::from(width);
            while run < runs.len() && before + u64::from(runs[run].length) <= start {
                before += u64::from(runs[run].length);
                run += 1;
            }
            row_starts.push((run as u32, (start - before) as u32));
        }
        let mirror = setup.mirror();
        Self {
            width,
            height,
            runs,
            row_starts,
            columns_from_max_x: !mirror.x,
            rows_from_max_y: mirror.y,
            z_mm: raster.z_mm(),
            raster: setup.clone(),
        }
    }

    /// Printer pixels across the plate's X, and along its Y.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The grey each rasterised grey reads back as from a file of `format`
    /// (#429); every grey as it is when no format is picked.
    pub(crate) fn stored_greys(&self, format: Option<PrinterFileFormat>) -> [u8; 256] {
        match format {
            Some(format) => sc_sla_formats::stored_greys(format, &self.raster),
            None => std::array::from_fn(|grey| grey as u8),
        }
    }

    /// The raster row holding the plate's row `row`, counted from the
    /// plate's back edge.
    fn raster_row(&self, row: u32) -> usize {
        if self.rows_from_max_y {
            row as usize
        } else {
            (self.height - 1 - row) as usize
        }
    }

    /// Hands each run of plate row `row` (from the back) within plate columns
    /// `[x0, x1)` (from the minimum X) to `each` as `(first column, past the
    /// last column, grey)`.
    fn for_each_segment(&self, row: u32, x0: u32, x1: u32, mut each: impl FnMut(u32, u32, u8)) {
        if row >= self.height || x0 >= x1 {
            return;
        }
        let (mut run, offset) = self.row_starts[self.raster_row(row)];
        let (a, b) = if self.columns_from_max_x {
            (self.width - x1, self.width - x0)
        } else {
            (x0, x1)
        };
        // Raster column the current run starts at (it may start on the row
        // before).
        let mut col = -i64::from(offset);
        while (run as usize) < self.runs.len() && col < i64::from(b) {
            let r = self.runs[run as usize];
            let end = col + i64::from(r.length);
            let lo = col.max(i64::from(a));
            let hi = end.min(i64::from(b));
            if lo < hi {
                let (lo, hi) = (lo as u32, hi as u32);
                if self.columns_from_max_x {
                    each(self.width - hi, self.width - lo, r.value);
                } else {
                    each(lo, hi, r.value);
                }
            }
            col = end;
            run += 1;
        }
    }

    /// The grey of the plate pixel at column `x` (from the minimum X) and row
    /// `row` (from the back).
    pub(crate) fn grey(&self, x: u32, row: u32) -> u8 {
        let mut grey = 0;
        self.for_each_segment(row, x, x + 1, |_, _, value| grey = value);
        grey
    }

    /// The greys of plate row `row` (from the back), every column, from the
    /// minimum X.
    #[cfg(test)]
    pub(crate) fn row(&self, row: u32) -> Vec<u8> {
        let mut out = vec![0; self.width as usize];
        self.for_each_segment(row, 0, self.width, |a, b, value| {
            out[a as usize..b as usize].fill(value);
        });
        out
    }
}

/// One axis of the overlay: the printer pixels `[start, start + len)` it
/// spans, `k` of them to a texel (zoomed out) or each `s` texels across
/// (zoomed in); one of the two is 1.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    start: u32,
    len: u32,
    k: u32,
    s: u32,
}

impl Axis {
    /// `[lo, hi)` of `count` printer pixels, drawn `screen_px` screen pixels
    /// to a printer pixel.
    fn new(lo: u32, hi: u32, screen_px: f64) -> Self {
        let len = hi.saturating_sub(lo).max(1);
        let (k, s) = if screen_px >= 1.0 {
            (1, (screen_px.floor() as u32).clamp(1, MAX_BLOCK_PX))
        } else {
            ((1.0 / screen_px.max(1e-6)).ceil() as u32, 1)
        };
        Self {
            start: lo,
            len,
            k,
            s,
        }
    }

    fn bins(&self) -> u32 {
        self.len.div_ceil(self.k)
    }

    fn texels(&self) -> u32 {
        self.bins() * self.s
    }

    /// Printer pixels in bin `bin`.
    fn bin_width(&self, bin: u32) -> u32 {
        self.k.min(self.len - bin * self.k)
    }

    /// The bin of printer pixel `p` (an absolute index on the axis).
    fn bin_of(&self, p: u32) -> u32 {
        (p - self.start) / self.k
    }

    /// Whether an overlay of this axis holds `other`'s pixels at its scale.
    fn holds(&self, other: &Axis) -> bool {
        self.k == other.k
            && self.s == other.s
            && self.start <= other.start
            && other.start + other.len <= self.start + self.len
    }

    fn grid_at(&self, texel: u32) -> bool {
        self.s >= GRID_FROM_PX && texel % self.s == self.s - 1
    }

    /// The bins printer pixels `[lo, hi)` reach.
    fn bins_of(&self, lo: u32, hi: u32) -> (u32, u32) {
        let end = self.start + self.len;
        let (lo, hi) = (lo.clamp(self.start, end), hi.clamp(self.start, end));
        if lo >= hi {
            return (0, 0);
        }
        (self.bin_of(lo), self.bin_of(hi - 1) + 1)
    }
}

/// What the overlay of the part on show looks like, and where it lies on the
/// plate, as fractions of the plate (the rows from its back).
pub(crate) struct Tile {
    pub(crate) image: SharedPixelBuffer<Rgba8Pixel>,
    /// Each texel's grey, before it is coloured.
    #[cfg(test)]
    pub(crate) greys: Vec<u8>,
    pub(crate) u0: f32,
    pub(crate) v0: f32,
    pub(crate) u1: f32,
    pub(crate) v1: f32,
}

/// The part of the pane's image on show: fractions of the image from its
/// left and top edges, and the image's size on screen, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    pub(crate) u0: f32,
    pub(crate) v0: f32,
    pub(crate) u1: f32,
    pub(crate) v1: f32,
    pub(crate) shown_width: f32,
    pub(crate) shown_height: f32,
}

/// The printer pixels `view` reaches of `layer`, and how many texels each
/// gets (or how many a texel takes), across and along. `None` for a view of
/// nothing.
fn axes(layer: &PixelLayer, view: View, margin: f32) -> Option<(Axis, Axis)> {
    let (w, h) = layer.size();
    if view.shown_width <= 0.0 || view.shown_height <= 0.0 || view.u1 <= view.u0 {
        return None;
    }
    let seen = view;
    let (du, dv) = ((view.u1 - view.u0) * margin, (view.v1 - view.v0) * margin);
    let view = View {
        u0: view.u0 - du,
        u1: view.u1 + du,
        v0: view.v0 - dv,
        v1: view.v1 + dv,
        ..view
    };
    let span = |a: f32, b: f32, count: u32| {
        let lo = (f64::from(a.clamp(0.0, 1.0)) * f64::from(count)).floor() as u32;
        let hi = (f64::from(b.clamp(0.0, 1.0)) * f64::from(count)).ceil() as u32;
        (lo.min(count.saturating_sub(1)), hi.clamp(lo + 1, count))
    };
    let (x0, x1) = span(view.u0, view.u1, w);
    let (y0, y1) = span(view.v0, view.v1, h);
    let mut ax = Axis::new(x0, x1, f64::from(view.shown_width) / f64::from(w));
    let mut ay = Axis::new(y0, y1, f64::from(view.shown_height) / f64::from(h));
    while u64::from(ax.texels()) * u64::from(ay.texels()) > MAX_TILE_TEXELS {
        if margin > 0.0 {
            // Fewer pixels round the view rather than coarser ones.
            return axes(layer, seen, if margin > 0.2 { margin / 2.0 } else { 0.0 });
        }
        if ax.s > 1 || ay.s > 1 {
            ax.s = (ax.s / 2).max(1);
            ay.s = (ay.s / 2).max(1);
        } else {
            ax.k *= 2;
            ay.k *= 2;
        }
    }

    Some((ax, ay))
}

/// The overlay of `layer` for `view`: the printer pixels it reaches, one
/// texel for several zoomed out, a block of texels each zoomed in, each
/// pixel's grey the one `greys` says the file stores for it, with
/// `highlight`'s printer pixels (`[x0, y0, x1, y1)`, plate columns from the
/// minimum X and rows from the back) drawn in the review's highlight (#409).
/// `None` for a view of nothing.
pub(crate) fn tile(
    layer: &PixelLayer,
    view: View,
    highlight: &[[u32; 4]],
    greys: &[u8; 256],
) -> Option<Tile> {
    Some(tile_of(layer, axes(layer, view, 0.0)?, highlight, greys))
}

/// The overlay of `layer`'s pixels on `axes`, as [`tile`] makes it.
fn tile_of(
    layer: &PixelLayer,
    (ax, ay): (Axis, Axis),
    highlight: &[[u32; 4]],
    greys: &[u8; 256],
) -> Tile {
    let (w, h) = layer.size();
    let (x0, x1, y0) = (ax.start, ax.start + ax.len, ay.start);

    let (bx, by) = (ax.bins() as usize, ay.bins() as usize);
    // The mean grey of each bin, as 0..=255.
    let mut means = vec![0u8; bx * by];
    let mut sums = vec![0u32; bx];
    for bin_row in 0..by {
        sums.fill(0);
        let first = y0 + bin_row as u32 * ay.k;
        let rows = ay.bin_width(bin_row as u32);
        for row in first..first + rows {
            layer.for_each_segment(row, x0, x1, |a, b, value| {
                let value = greys[usize::from(value)];
                if value == 0 {
                    return;
                }
                let mut p = a;
                while p < b {
                    let bin = ax.bin_of(p);
                    let bin_end = ax.start + (bin + 1) * ax.k;
                    let upto = bin_end.min(b);
                    sums[bin as usize] += u32::from(value) * (upto - p);
                    p = upto;
                }
            });
        }
        for (bin, sum) in sums.iter().enumerate() {
            let count = ax.bin_width(bin as u32) * rows;
            means[bin_row * bx + bin] = ((*sum + count / 2) / count) as u8;
        }
    }
    let mut marked = vec![false; bx * by];
    for &[hx0, hy0, hx1, hy1] in highlight {
        let (c0, c1) = ax.bins_of(hx0, hx1);
        let (r0, r1) = ay.bins_of(hy0, hy1);
        for r in r0..r1 {
            marked[r as usize * bx + c0 as usize..r as usize * bx + c1 as usize].fill(true);
        }
    }

    let (tw, th) = (ax.texels(), ay.texels());
    let mut image = SharedPixelBuffer::<Rgba8Pixel>::new(tw, th);
    let pixels = image.make_mut_slice();
    // Opaque colours: the pixel's grey over the pane's background, a third
    // darker on the grid.
    let mix = |grey: u8, of: [u8; 3]| -> [u8; 3] {
        std::array::from_fn(|i| {
            let (lo, hi) = (i32::from(BACKGROUND_RGB[i]), i32::from(of[i]));
            (lo + (hi - lo) * i32::from(grey) / 255) as u8
        })
    };
    let opaque = |[r, g, b]: [u8; 3]| Rgba8Pixel { r, g, b, a: 255 };
    let darker = |c: [u8; 3]| c.map(|v| (u16::from(v) * 7 / 10) as u8);
    let fill: Vec<[u8; 3]> = (0..=u8::MAX).map(|g| mix(g, FILL_RGB)).collect();
    let empty_grid = mix(22, [255, 255, 255]);
    let mark = crate::slice_cap::HIGHLIGHT_RGB;
    #[cfg(test)]
    let mut texel_greys = Vec::with_capacity((tw * th) as usize);
    for ty in 0..th {
        let bin_row = (ty / ay.s) as usize;
        let grid_row = ay.grid_at(ty);
        for tx in 0..tw {
            let bin = bin_row * bx + (tx / ax.s) as usize;
            let grid = grid_row || ax.grid_at(tx);
            let grey = means[bin];
            #[cfg(test)]
            texel_greys.push(grey);
            let colour = match (marked[bin], grid) {
                (true, false) => mark,
                (true, true) => darker(mark),
                (false, false) => fill[usize::from(grey)],
                (false, true) if grey == 0 => empty_grid,
                (false, true) => darker(fill[usize::from(grey)]),
            };
            pixels[(ty * tw + tx) as usize] = opaque(colour);
        }
    }
    Tile {
        image,
        #[cfg(test)]
        greys: texel_greys,
        u0: x0 as f32 / w as f32,
        u1: (ax.start + ax.len) as f32 / w as f32,
        v0: y0 as f32 / h as f32,
        v1: (ay.start + ay.len) as f32 / h as f32,
    }
}

/// The printer pixels `rects` reach (`[x0, y0, x1, y1]` in printer-frame mm,
/// centred on the plate, as the layer's contours), as `[x0, y0, x1, y1)`
/// plate columns from the minimum X and rows from the back.
fn highlight_pixels(rects: &[[f64; 4]], plate_mm: (f64, f64), size: (u32, u32)) -> Vec<[u32; 4]> {
    let (w, h) = size;
    let (sx, sy) = (f64::from(w) / plate_mm.0, f64::from(h) / plate_mm.1);
    let span = |lo: f64, hi: f64, scale: f64, half: f64, count: u32| {
        // A rectangle edge on a pixel edge reaches no further.
        let a = ((lo + half) * scale + 1e-6).floor().max(0.0) as u32;
        let b = ((hi + half) * scale - 1e-6).ceil().max(0.0) as u32;
        (
            a.min(count),
            b.min(count).max(a.min(count) + u32::from(a < count)),
        )
    };
    rects
        .iter()
        .map(|&[x0, y0, x1, y1]| {
            let (c0, c1) = span(x0, x1, sx, plate_mm.0 / 2.0, w);
            let (r0, r1) = span(y0, y1, sy, plate_mm.1 / 2.0, h);
            // Rows from the back.
            [c0, h - r1, c1, h - r0]
        })
        .collect()
}

/// What a rasterised layer is of: the slice it was cut in, the layer, and the
/// printer and anti-aliasing its file is written with.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    stack: usize,
    layer: u32,
    printer: PrinterId,
    aa: AaLevel,
}

type Slot = Arc<Mutex<Option<(Key, PixelLayer)>>>;

/// Drives the layer pane's real pixels in one window.
pub(crate) struct LayerPixels {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    ctrl: Rc<ViewportController>,
    /// Where the printer file's format is picked.
    panel: Rc<SlicePanelBridge>,
    /// The printer of the held slice, loaded once.
    printer: RefCell<Option<(PrinterId, Arc<PrinterProfile>)>>,
    /// The layer rasterised last.
    layer: RefCell<Option<(Key, Arc<PixelLayer>)>>,
    /// The layer being rasterised.
    running: RefCell<Option<Key>>,
    slot: Slot,
    view: Cell<Option<View>>,
    generation: Cell<i32>,
    /// The overlay on show reaches past its view.
    margined: Cell<bool>,
    /// Makes the overlay again while the view moves, now and then.
    throttle: slint::Timer,
    /// Makes the overlay with its margin once the view keeps still.
    settle: slint::Timer,
    /// What the overlay on show was made of: the layer, its view's pixels
    /// and scale, the greys and the highlight.
    shown: RefCell<Option<Shown>>,
    me: std::rc::Weak<Self>,
}

/// What an overlay was made of: equal, it would come out the same.
#[derive(Clone, Debug, PartialEq)]
struct Shown {
    key: Key,
    axes: (Axis, Axis),
    greys: Vec<u8>,
    highlight: Vec<[u32; 4]>,
}

impl LayerPixels {
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        ctrl: Rc<ViewportController>,
        panel: Rc<SlicePanelBridge>,
    ) -> Rc<Self> {
        let pixels = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            margined: Cell::new(false),
            throttle: slint::Timer::default(),
            settle: slint::Timer::default(),
            shown: RefCell::new(None),
            ui: ui.as_weak(),
            document,
            ctrl,
            panel,
            printer: RefCell::new(None),
            layer: RefCell::new(None),
            running: RefCell::new(None),
            slot: Arc::new(Mutex::new(None)),
            view: Cell::new(None),
            generation: Cell::new(0),
        });
        {
            let pixels = pixels.clone();
            ui.on_layer_view_changed(move |u0, v0, u1, v1, shown_width, shown_height| {
                pixels.on_view(View {
                    u0,
                    v0,
                    u1,
                    v1,
                    shown_width,
                    shown_height,
                });
            });
        }
        {
            let pixels = pixels.clone();
            ui.on_layer_pixels_ready(move || pixels.finish());
        }
        {
            let pixels = pixels.clone();
            ui.on_layer_pixel_readout(move |u, v, _generation| pixels.readout(u, v));
        }
        pixels
    }

    /// Keeps the pane's view. While it moves the overlay is made again at
    /// most every [`THROTTLE`], and only when the view leaves the part it
    /// was made for; once it keeps still for [`SETTLE`], with its margin. A
    /// zoom step reports the view several times over: by the time a timer
    /// fires, it has settled.
    fn on_view(&self, view: View) {
        self.view.set(Some(view));
        if !self.throttle.running() {
            let me = self.me.clone();
            self.throttle
                .start(slint::TimerMode::SingleShot, THROTTLE, move || {
                    if let Some(pixels) = me.upgrade() {
                        pixels.refresh(false);
                    }
                });
        }
        let me = self.me.clone();
        self.settle
            .start(slint::TimerMode::SingleShot, SETTLE, move || {
                if let Some(pixels) = me.upgrade() {
                    pixels.refresh(true);
                }
            });
    }

    /// The held slice's contours, printer and anti-aliasing, and the layer on
    /// show.
    fn target(&self) -> Option<(PlateContours, Arc<PrinterProfile>, Key)> {
        let ui = self.ui.upgrade()?;
        let held = self.document.held_slice()?;
        let layers = held.contours.stack().layers().len();
        let layer = ui.get_selected_layer_index().round().max(0.0) as u32;
        if layer as usize >= layers {
            return None;
        }
        let printer = self.printer_profile(&held.params.printer)?;
        let key = Key {
            stack: Arc::as_ptr(&held.contours.shared_stack()) as usize,
            layer,
            printer: held.params.printer.clone(),
            aa: held.params.aa_level,
        };
        Some((held.contours, printer, key))
    }

    fn printer_profile(&self, id: &PrinterId) -> Option<Arc<PrinterProfile>> {
        if let Some((known, profile)) = self.printer.borrow().as_ref()
            && known == id
        {
            return Some(profile.clone());
        }
        let profile = Arc::new(
            sc_print_profiles::ProfileLibraries::platform_default()
                .load_printer_by_id(id)
                .ok()?,
        );
        *self.printer.borrow_mut() = Some((id.clone(), profile.clone()));
        Some(profile)
    }

    /// Shows the part on show at the printer's resolution once its layer is
    /// rasterised, and starts rasterising it otherwise.
    fn refresh(&self, settled: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some((contours, printer, key)) = self.target() else {
            ui.set_layer_pixel_shown(false);
            *self.shown.borrow_mut() = None;
            return;
        };
        let res = printer.hardware.pixel_resolution();
        ui.set_layer_pixel_columns(i32::try_from(res.x()).unwrap_or(i32::MAX));
        let ready = self
            .layer
            .borrow()
            .as_ref()
            .filter(|(known, _)| *known == key)
            .map(|(_, layer)| layer.clone());
        match (ready, self.view.get()) {
            (Some(layer), Some(view)) => self.show(&ui, &layer, view, key, settled),
            (Some(_), None) => {}
            (None, _) => {
                ui.set_layer_pixel_shown(false);
                *self.shown.borrow_mut() = None;
                self.start(contours, &printer, key);
            }
        }
    }

    /// Shows the overlay of `view`, unless the one on show holds it already.
    /// It reaches past the view by [`MARGIN`] once `settled`, or while the
    /// view pans at the scale of the one on show; a zoom gets the view alone,
    /// which is quicker.
    fn show(&self, ui: &MainWindow, layer: &PixelLayer, view: View, key: Key, settled: bool) {
        let plate = printer_plate_mm(self.printer.borrow().as_ref().map(|(_, p)| p.as_ref()));
        let highlight = plate
            .map(|plate| {
                highlight_pixels(
                    &self.ctrl.review_highlight_rects(layer.z_mm),
                    plate,
                    layer.size(),
                )
            })
            .unwrap_or_default();
        let greys = layer.stored_greys(self.panel.selected_print_format());
        let Some(seen) = axes(layer, view, 0.0) else {
            ui.set_layer_pixel_shown(false);
            *self.shown.borrow_mut() = None;
            return;
        };
        let (fits, panning) = match self.shown.borrow().as_ref() {
            Some(on_show)
                if on_show.key == key
                    && on_show.greys[..] == greys[..]
                    && on_show.highlight == highlight =>
            {
                let (x, y) = on_show.axes;
                (
                    x.holds(&seen.0) && y.holds(&seen.1),
                    (x.k, x.s, y.k, y.s) == (seen.0.k, seen.0.s, seen.1.k, seen.1.s),
                )
            }
            _ => (false, false),
        };
        if fits && (self.margined.get() || !settled) {
            // The overlay on show holds the view: nothing to make again.
            return;
        }
        let margined = settled || panning;
        let margin = if margined { MARGIN } else { 0.0 };
        let Some(axes) = axes(layer, view, margin) else {
            return;
        };
        let tile = tile_of(layer, axes, &highlight, &greys);
        self.margined.set(margined);
        *self.shown.borrow_mut() = Some(Shown {
            key,
            axes,
            greys: greys.to_vec(),
            highlight,
        });
        ui.set_layer_pixel_tile(slint::Image::from_rgba8(tile.image));
        ui.set_layer_pixel_u0(tile.u0);
        ui.set_layer_pixel_v0(tile.v0);
        ui.set_layer_pixel_u1(tile.u1);
        ui.set_layer_pixel_v1(tile.v1);
        ui.set_layer_pixel_shown(true);
    }

    /// Rasterises layer `key` of `contours` on a worker thread, unless a
    /// layer is being rasterised already: when it is done, the layer on show
    /// then is rasterised.
    fn start(&self, contours: PlateContours, printer: &PrinterProfile, key: Key) {
        if self.running.borrow().is_some() {
            return;
        }
        let setup = sc_print_glue::export::export_raster(&contours, printer, key.aa);
        let stack: Arc<ContourStack> = contours.shared_stack();
        let slot = self.slot.clone();
        let ui = self.ui.clone();
        *self.running.borrow_mut() = Some(key.clone());
        let spawned = std::thread::Builder::new()
            .name("layer-pixels".into())
            .spawn(move || {
                let raster = layer_pixels(&stack.layers()[key.layer as usize], &setup);
                let layer = PixelLayer::new(&raster, &setup);
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((key, layer));
                let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_layer_pixels_ready());
            });
        if spawned.is_err() {
            *self.running.borrow_mut() = None;
        }
    }

    /// Takes a rasterised layer on the UI thread.
    fn finish(&self) {
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        *self.running.borrow_mut() = None;
        if let Some((key, layer)) = taken {
            *self.layer.borrow_mut() = Some((key, Arc::new(layer)));
            self.generation.set(self.generation.get().wrapping_add(1));
            if let Some(ui) = self.ui.upgrade() {
                ui.set_layer_pixel_generation(self.generation.get());
            }
        }
        self.refresh(true);
    }

    /// The place and grey of the printer pixel at `(u, v)`, fractions of the
    /// pane's image from its left and top edges; empty off the plate or
    /// before the layer on show is rasterised.
    fn readout(&self, u: f32, v: f32) -> SharedString {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return SharedString::new();
        }
        let Some((_, _, key)) = self.target() else {
            return SharedString::new();
        };
        let layer = self.layer.borrow();
        let Some((_, layer)) = layer.as_ref().filter(|(known, _)| *known == key) else {
            return SharedString::new();
        };
        let (w, h) = layer.size();
        let x = ((f64::from(u) * f64::from(w)) as u32).min(w - 1);
        let row = ((f64::from(v) * f64::from(h)) as u32).min(h - 1);
        let grey =
            layer.stored_greys(self.panel.selected_print_format())[usize::from(layer.grey(x, row))];
        // Counted from the plate's front left corner, as the plate's rulers.
        ctr!(
            "LayerView",
            "Pixel {} × {}  ·  grey {}",
            x,
            h - 1 - row,
            grey
        )
    }
}

/// The plate of `printer`, in millimetres.
fn printer_plate_mm(printer: Option<&PrinterProfile>) -> Option<(f64, f64)> {
    let volume = printer?.hardware.build_volume();
    Some((volume.x_mm(), volume.y_mm()))
}

#[cfg(test)]
#[path = "tests_layer_pixels.rs"]
mod tests;
