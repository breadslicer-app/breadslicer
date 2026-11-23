//! How a support tip a few pixels wide comes out of the rasteriser (#724).

use sc_sl1cer::contour::{ClosedRing, ContourLayer, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{AaLevel, Mirror, PixelResolution, PositiveFiniteMm};
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::scanline::rasterize_layer;

const SIDE_PX: u32 = 12;

/// A plate of `SIDE_PX` pixels a side, one millimetre each.
fn plate() -> PixelMap {
    let side = PositiveFiniteMm::new(f64::from(SIDE_PX)).unwrap();
    PixelMap::new(
        side,
        side,
        PixelResolution::new(SIDE_PX, SIDE_PX).unwrap(),
        Mirror::default(),
    )
}

/// A round section `d` pixels across centred at `(x, y)`, in pixels from
/// the plate's centre.
fn disc(d: f64, x: f64, y: f64) -> ContourLayer {
    let ring = ClosedRing::new(
        (0..48)
            .map(|k| {
                let a = std::f64::consts::TAU * f64::from(k) / 48.0;
                Point2D::new(x + 0.5 * d * a.cos(), y + 0.5 * d * a.sin())
            })
            .collect(),
    );
    ContourLayer::new(0.05, vec![Polygon::new(ring, vec![]).unwrap()])
}

/// Over every placement of the section on the pixel grid: the brightest
/// pixel of its worst placement, and the least light it gets in all (in
/// full pixels).
fn worst(d: f64, aa: AaLevel) -> (u8, f64) {
    let pm = plate();
    let (mut brightest, mut light) = (255u8, f64::MAX);
    for i in 0..20 {
        for j in 0..20 {
            let (x, y) = (f64::from(i) / 20.0, f64::from(j) / 20.0);
            let raster = rasterize_layer(&disc(d, x, y), &pm, aa, 0.0);
            let max = raster.runs().iter().map(|r| r.value).max().unwrap_or(0);
            let sum: f64 = raster
                .runs()
                .iter()
                .map(|r| f64::from(r.value) * f64::from(r.length) / 255.0)
                .sum();
            brightest = brightest.min(max);
            light = light.min(sum);
        }
    }
    (brightest, light)
}

#[test]
#[ignore = "prints a table; run by hand for #724"]
fn thin_tips_on_the_pixel_grid() {
    for d in [1.0, 1.31, 1.5, 2.0, 2.5, 2.83, 3.0, 3.5, 4.0] {
        let row: Vec<String> = [AaLevel::Off, AaLevel::X2, AaLevel::X4, AaLevel::X8]
            .into_iter()
            .map(|aa| {
                let (b, l) = worst(d, aa);
                format!("{b:>3} {l:>5.2}")
            })
            .collect();
        eprintln!(
            "d {d:>4} px | off {} | x2 {} | x4 {} | x8 {}",
            row[0], row[1], row[2], row[3]
        );
    }
}
