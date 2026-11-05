/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The region machinery against a brute-force reference.
//!
//! Random plates are painted into plain bitmaps, labelled pixel by pixel with
//! a flood fill and compared with [`PlateRegions`] field by field: regions,
//! role counts, links, groups across layers, and the same result whatever
//! the thread count. A few hand-made cases pin the frame of the focus point
//! with polygons given in millimetres, so they do not reuse the conversion
//! under test.

use std::collections::{BTreeMap, VecDeque};

use sc_print_primitives::{Mirror, PixelResolution, PositiveFiniteMm};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::LayerSchedule;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rle::{LayerRaster, RleRun};

use super::*;
use crate::input::ReviewLeaf;
use crate::test_support::{Fixture, FixtureBuilder, RAW, Shape};
use crate::union_find::RegionRef;

/// Screen of the random plates: odd sizes, so nothing lines up by chance.
const W: u32 = 37;
const H: u32 = 23;

/// A small deterministic generator (xorshift64*), so a failure replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n)) as u32
    }
}

/// A shape drawn on a random plate, in whole pixels.
#[derive(Clone, Copy, Debug)]
enum Drawn {
    Rect {
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    },
    Frame {
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
        wall: u32,
    },
}

impl Drawn {
    fn random(rng: &mut Rng) -> Self {
        let (x0, y0) = (rng.below(W), rng.below(H));
        if rng.below(4) == 0 {
            let wall = 1 + rng.below(2);
            let x1 = (x0 + 2 * wall + 1 + rng.below(8)).min(W);
            let y1 = (y0 + 2 * wall + 1 + rng.below(8)).min(H);
            if x1 - x0 > 2 * wall && y1 - y0 > 2 * wall {
                return Self::Frame {
                    x0,
                    y0,
                    x1,
                    y1,
                    wall,
                };
            }
        }
        if rng.below(40) == 0 {
            // The whole screen: a layer without any air.
            return Self::Rect {
                x0: 0,
                y0: 0,
                x1: W,
                y1: H,
            };
        }
        // Mostly small, often single pixels, so corner contacts are common.
        let w = if rng.below(3) == 0 {
            1
        } else {
            1 + rng.below(9)
        };
        let h = if rng.below(3) == 0 {
            1
        } else {
            1 + rng.below(6)
        };
        Self::Rect {
            x0,
            y0,
            x1: (x0 + w).min(W),
            y1: (y0 + h).min(H),
        }
    }

    fn shape(self) -> Shape {
        let f = f64::from;
        match self {
            Self::Rect { x0, y0, x1, y1 } => Shape::rect(f(x0), f(y0), f(x1), f(y1)),
            Self::Frame {
                x0,
                y0,
                x1,
                y1,
                wall,
            } => Shape::frame(f(x0), f(y0), f(x1), f(y1), f(wall)),
        }
    }

    fn covers(self, x: u32, y: u32) -> bool {
        let inside = |x0: u32, y0: u32, x1: u32, y1: u32| x0 <= x && x < x1 && y0 <= y && y < y1;
        match self {
            Self::Rect { x0, y0, x1, y1 } => inside(x0, y0, x1, y1),
            Self::Frame {
                x0,
                y0,
                x1,
                y1,
                wall,
            } => inside(x0, y0, x1, y1) && !inside(x0 + wall, y0 + wall, x1 - wall, y1 - wall),
        }
    }
}

/// The shapes of one layer, by role.
#[derive(Clone, Debug, Default)]
struct DrawnLayer {
    model: Vec<Drawn>,
    support: Vec<Drawn>,
}

/// A random plate of `layers` layers. Layers often repeat or slightly change
/// the one below, so bodies and cavities span several layers.
fn random_plate(seed: u64, layers: usize) -> Vec<DrawnLayer> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut plate: Vec<DrawnLayer> = Vec::with_capacity(layers);
    let mut current = DrawnLayer::default();
    for _ in 0..layers {
        match rng.below(6) {
            0 => current = DrawnLayer::default(),
            1 | 2 => {}
            3 => current.model.push(Drawn::random(&mut rng)),
            4 => {
                if !current.model.is_empty() {
                    let at = rng.below(current.model.len() as u32) as usize;
                    current.model.remove(at);
                }
                current.support.push(Drawn::random(&mut rng));
            }
            _ => {
                current = DrawnLayer::default();
                for _ in 0..rng.below(7) {
                    current.model.push(Drawn::random(&mut rng));
                }
                for _ in 0..rng.below(3) {
                    current.support.push(Drawn::random(&mut rng));
                }
            }
        }
        plate.push(current.clone());
    }
    plate
}

fn build(plate: &[DrawnLayer]) -> Fixture {
    let mut builder = FixtureBuilder::new(plate.len())
        .screen(W, H, 0.1)
        .max_z_mm(1000.0);
    for (k, layer) in plate.iter().enumerate() {
        for drawn in &layer.model {
            builder = builder.model(k..=k, drawn.shape());
        }
        for drawn in &layer.support {
            builder = builder.shape("Second", k..=k, drawn.shape());
        }
    }
    builder.build()
}

/// One layer as bitmaps, row-major.
struct Bitmaps {
    model: Vec<bool>,
    support: Vec<bool>,
}

impl Bitmaps {
    fn paint(layer: &DrawnLayer) -> Self {
        let paint = |shapes: &[Drawn]| -> Vec<bool> {
            (0..H)
                .flat_map(|y| (0..W).map(move |x| (x, y)))
                .map(|(x, y)| shapes.iter().any(|s| s.covers(x, y)))
                .collect()
        };
        Self {
            model: paint(&layer.model),
            support: paint(&layer.support),
        }
    }

    fn solid(&self) -> Vec<bool> {
        self.model
            .iter()
            .zip(&self.support)
            .map(|(m, s)| *m || *s)
            .collect()
    }
}

const NONE: u32 = u32::MAX;

/// Flood-fill labels of `mask`, numbered in raster scan order.
fn flood(mask: &[bool], eight: bool) -> (Vec<u32>, usize) {
    let mut labels = vec![NONE; mask.len()];
    let mut count = 0u32;
    let mut queue = VecDeque::new();
    for start in 0..mask.len() {
        if !mask[start] || labels[start] != NONE {
            continue;
        }
        labels[start] = count;
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            let (x, y) = ((i as u32 % W) as i64, (i as u32 / W) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if (dx, dy) == (0, 0) || (!eight && dx != 0 && dy != 0) {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= i64::from(W) || ny >= i64::from(H) {
                        continue;
                    }
                    let j = (ny * i64::from(W) + nx) as usize;
                    if mask[j] && labels[j] == NONE {
                        labels[j] = count;
                        queue.push_back(j);
                    }
                }
            }
        }
        count += 1;
    }
    (labels, count as usize)
}

/// Checks the regions of one kind of one layer against the reference.
#[allow(clippy::too_many_arguments)]
fn check_regions(
    what: &str,
    regions: &[Region],
    labels: &[u32],
    count: usize,
    linked: bool,
    below: Option<&[bool]>,
    above: Option<&[bool]>,
) {
    assert_eq!(regions.len(), count, "{what}: region count");
    for (r, region) in regions.iter().enumerate() {
        let pixels: Vec<usize> = (0..labels.len())
            .filter(|&i| labels[i] == r as u32)
            .collect();
        let xy = |i: usize| ((i as u32) % W, (i as u32) / W);
        let area = pixels.len() as u64;
        assert_eq!(region.area_px, area, "{what} {r}: area");
        let bbox = PixelBox {
            x0: pixels.iter().map(|&i| xy(i).0).min().unwrap(),
            y0: pixels.iter().map(|&i| xy(i).1).min().unwrap(),
            x1: pixels.iter().map(|&i| xy(i).0).max().unwrap() + 1,
            y1: pixels.iter().map(|&i| xy(i).1).max().unwrap() + 1,
        };
        assert_eq!(region.bbox, bbox, "{what} {r}: bbox");
        let sx: u64 = pixels.iter().map(|&i| 2 * u64::from(xy(i).0) + 1).sum();
        let sy: u64 = pixels.iter().map(|&i| 2 * u64::from(xy(i).1) + 1).sum();
        let centroid = [sx as f64 / (2 * area) as f64, sy as f64 / (2 * area) as f64];
        assert_eq!(region.centroid_px, centroid, "{what} {r}: centroid");
        let border = pixels.iter().any(|&i| {
            let (x, y) = xy(i);
            x == 0 || y == 0 || x + 1 == W || y + 1 == H
        });
        assert_eq!(region.touches_border, border, "{what} {r}: border");
        let count_in = |mask: Option<&[bool]>| {
            mask.map_or(0, |m| pixels.iter().filter(|&&i| m[i]).count() as u64)
        };
        if linked {
            assert_eq!(region.below_px, count_in(below), "{what} {r}: below");
            assert_eq!(region.above_px, count_in(above), "{what} {r}: above");
        }
        let [ix, iy] = region.inside_px;
        let inside = (iy * W + ix) as usize;
        assert_eq!(labels[inside], r as u32, "{what} {r}: inside pixel");
        let distance = |i: usize| {
            let (x, y) = xy(i);
            let dx = f64::from(x) + 0.5 - centroid[0];
            let dy = f64::from(y) + 0.5 - centroid[1];
            dx * dx + dy * dy
        };
        let nearest = pixels
            .iter()
            .map(|&i| distance(i))
            .fold(f64::INFINITY, f64::min);
        assert!(
            (distance(inside) - nearest).abs() < 1e-9,
            "{what} {r}: inside pixel is not the nearest"
        );
    }
}

/// The overlaps of every lower and upper label pair.
fn reference_links(lower: &[u32], upper: &[u32]) -> Vec<Overlap> {
    let mut pairs: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for (a, b) in lower.iter().zip(upper) {
        if *a != NONE && *b != NONE {
            *pairs.entry((*a, *b)).or_default() += 1;
        }
    }
    pairs
        .into_iter()
        .map(|((lower, upper), pixels)| Overlap {
            lower,
            upper,
            pixels,
        })
        .collect()
}

/// 3D components of the stacked masks: in-layer neighbours by `eight`, and
/// straight up and down.
fn components_3d(masks: &[Vec<bool>], eight: bool) -> Vec<Vec<u32>> {
    let plane = (W * H) as usize;
    let mut labels = vec![vec![NONE; plane]; masks.len()];
    let mut count = 0u32;
    let mut queue = VecDeque::new();
    for k0 in 0..masks.len() {
        for start in 0..plane {
            if !masks[k0][start] || labels[k0][start] != NONE {
                continue;
            }
            labels[k0][start] = count;
            queue.push_back((k0, start));
            while let Some((k, i)) = queue.pop_front() {
                let (x, y) = ((i as u32 % W) as i64, (i as u32 / W) as i64);
                let mut visit = |k: usize, j: usize, queue: &mut VecDeque<(usize, usize)>| {
                    if masks[k][j] && labels[k][j] == NONE {
                        labels[k][j] = count;
                        queue.push_back((k, j));
                    }
                };
                if k > 0 {
                    visit(k - 1, i, &mut queue);
                }
                if k + 1 < masks.len() {
                    visit(k + 1, i, &mut queue);
                }
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        if (dx, dy) == (0, 0) || (!eight && dx != 0 && dy != 0) {
                            continue;
                        }
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= i64::from(W) || ny >= i64::from(H) {
                            continue;
                        }
                        visit(k, (ny * i64::from(W) + nx) as usize, &mut queue);
                    }
                }
            }
            count += 1;
        }
    }
    labels
}

/// Checks that `groups` partitions the regions exactly as the 3D components
/// do, and that groups are numbered and listed by their lowest member.
fn check_groups(what: &str, groups: &Groups, layer_labels: &[Vec<u32>], comps: &[Vec<u32>]) {
    let mut group_to_comp: BTreeMap<usize, u32> = BTreeMap::new();
    let mut comp_to_group: BTreeMap<u32, usize> = BTreeMap::new();
    for (k, labels) in layer_labels.iter().enumerate() {
        for (i, &label) in labels.iter().enumerate() {
            if label == NONE {
                continue;
            }
            let group = groups.group_of(RegionRef {
                layer: k,
                index: label,
            });
            let comp = comps[k][i];
            assert_eq!(
                *group_to_comp.entry(group).or_insert(comp),
                comp,
                "{what}: group {group} spans two bodies"
            );
            assert_eq!(
                *comp_to_group.entry(comp).or_insert(group),
                group,
                "{what}: body {comp} split in two groups"
            );
        }
    }
    assert_eq!(groups.len(), group_to_comp.len(), "{what}: group count");
    let mut previous_first: Option<RegionRef> = None;
    for (g, members) in groups.iter().enumerate() {
        assert!(!members.is_empty(), "{what}: empty group {g}");
        assert!(
            members.windows(2).all(|pair| pair[0] < pair[1]),
            "{what}: members of group {g} out of order"
        );
        assert!(
            previous_first.is_none_or(|first| first < members[0]),
            "{what}: groups not numbered by their lowest member"
        );
        previous_first = Some(members[0]);
        for member in members {
            assert_eq!(groups.group_of(*member), g, "{what}: member lists");
        }
    }
}

fn compute_with_threads(input: &ReviewInput<'_>, threads: usize) -> PlateRegions {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("a thread pool")
        .install(|| PlateRegions::compute(input, &RAW))
}

fn check_plate(seed: u64, layers: usize) {
    let plate = random_plate(seed, layers);
    let fixture = build(&plate);
    let input = fixture.input();
    // One thread, then others: chunks of other sizes.
    let regions = compute_with_threads(&input, 1);
    let bitmaps: Vec<Bitmaps> = plate.iter().map(Bitmaps::paint).collect();
    let solids: Vec<Vec<bool>> = bitmaps.iter().map(Bitmaps::solid).collect();
    let airs: Vec<Vec<bool>> = solids
        .iter()
        .map(|s| s.iter().map(|p| !p).collect())
        .collect();
    let solid_labels: Vec<(Vec<u32>, usize)> = solids.iter().map(|m| flood(m, true)).collect();
    let air_labels: Vec<(Vec<u32>, usize)> = airs.iter().map(|m| flood(m, false)).collect();

    for k in 0..layers {
        let what = format!("seed {seed} layer {k}");
        let layer = regions.layer(k);
        let below = |masks: &[Vec<bool>]| (k > 0).then(|| masks[k - 1].clone());
        let above = |masks: &[Vec<bool>]| (k + 1 < layers).then(|| masks[k + 1].clone());
        // A pixel counts as resting on the layer below only when it is in the
        // region itself, which `check_regions` restricts to.
        check_regions(
            &format!("{what} solid"),
            &layer.solid,
            &solid_labels[k].0,
            solid_labels[k].1,
            true,
            below(&solids).as_deref(),
            above(&solids).as_deref(),
        );
        check_regions(
            &format!("{what} air"),
            &layer.air,
            &air_labels[k].0,
            air_labels[k].1,
            false,
            None,
            None,
        );
        if k == 0 {
            assert!(regions.solid_links(0).is_empty());
            assert!(regions.air_links(0).is_empty());
        } else {
            assert_eq!(
                regions.solid_links(k),
                reference_links(&solid_labels[k - 1].0, &solid_labels[k].0).as_slice(),
                "{what}: solid links"
            );
            // Only links with an enclosed region are kept.
            let enclosed = |layer: usize, index: u32| {
                !regions.layer(layer).air[index as usize].touches_border
            };
            let mut links = reference_links(&air_labels[k - 1].0, &air_labels[k].0);
            links.retain(|link| enclosed(k - 1, link.lower) || enclosed(k, link.upper));
            assert_eq!(regions.air_links(k), links.as_slice(), "{what}: air links");
        }
        // The per-layer pixels give the same regions, without the links.
        let mut alone = regions.layer(k).clone();
        alone.starts.clear();
        for region in alone.solid.iter_mut().chain(alone.air.iter_mut()) {
            region.below_px = 0;
            region.above_px = 0;
        }
        assert_eq!(
            fixture.raw_pixels(k).regions(W, H),
            alone,
            "{what}: pixels"
        );
    }

    let solid_groups = regions.group_solid(|_, _| true);
    let labels: Vec<Vec<u32>> = solid_labels.iter().map(|(l, _)| l.clone()).collect();
    check_groups(
        &format!("seed {seed} solid"),
        &solid_groups,
        &labels,
        &components_3d(&solids, true),
    );
    // The same plate on other thread counts (other batch sizes).
    for threads in [3, 8] {
        let other = compute_with_threads(&input, threads);
        assert_eq!(
            other.layers(),
            regions.layers(),
            "seed {seed}: {threads} threads"
        );
        for k in 0..layers {
            assert_eq!(other.solid_links(k), regions.solid_links(k));
            assert_eq!(other.air_links(k), regions.air_links(k));
        }
    }
}

#[test]
fn random_plates_match_a_flood_fill_reference() {
    for seed in 1..=60 {
        check_plate(seed, 9 + (seed as usize % 4) * 7);
    }
}

#[test]
fn label_runs_handles_rows_corners_and_gaps() {
    let run = Run::new;
    // Diagonal contact across a row boundary.
    let diagonal = [run(0, 0, 2), run(1, 2, 4)];
    assert_eq!(label_runs(&diagonal, Connectivity::Eight).1, 1);
    assert_eq!(label_runs(&diagonal, Connectivity::Four).1, 2);
    // One empty row between: never connected.
    let gap = [run(0, 0, 5), run(2, 0, 5)];
    assert_eq!(label_runs(&gap, Connectivity::Eight), (vec![0, 1], 2));
    // A U closed below: both arms join through the base, labelled by the
    // first run met.
    let u = [run(0, 0, 1), run(0, 4, 5), run(1, 0, 5)];
    assert_eq!(label_runs(&u, Connectivity::Four), (vec![0, 0, 0], 1));
    // A run on the next row that skips a run of the row above.
    let skip = [run(0, 0, 1), run(0, 3, 4), run(0, 8, 9), run(1, 7, 8)];
    assert_eq!(
        label_runs(&skip, Connectivity::Eight),
        (vec![0, 1, 2, 2], 3)
    );
    assert_eq!(label_runs(&skip, Connectivity::Four), (vec![0, 1, 2, 3], 4));
    // One wide run joining many runs above and below.
    let comb = [
        run(0, 0, 1),
        run(0, 2, 3),
        run(0, 4, 5),
        run(1, 0, 5),
        run(2, 1, 2),
        run(2, 3, 4),
    ];
    assert_eq!(label_runs(&comb, Connectivity::Four).1, 1);
    assert_eq!(label_runs(&[], Connectivity::Four), (vec![], 0));
}

#[test]
fn raster_runs_merge_lit_pixels_of_different_values() {
    // An anti-aliased edge: 128 then 255 then 64 on one row, then a run that
    // wraps into the next row with another value.
    let raster = LayerRaster::new(
        0.0,
        6,
        2,
        vec![
            RleRun {
                value: 0,
                length: 1,
            },
            RleRun {
                value: 128,
                length: 1,
            },
            RleRun {
                value: 255,
                length: 2,
            },
            RleRun {
                value: 64,
                length: 3,
            },
            RleRun {
                value: 255,
                length: 1,
            },
            RleRun {
                value: 0,
                length: 4,
            },
        ],
    );
    let runs = runs::runs_from_raster(&raster);
    assert_eq!(runs, vec![Run::new(0, 1, 6), Run::new(1, 0, 2)]);
    // One region, as the pixels are one stretch.
    assert_eq!(label_runs(&runs, SOLID_CONNECTIVITY).1, 1);
}

fn mm(v: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(v).unwrap()
}

/// A square given in plate millimetres, counter-clockwise.
fn square_mm(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
    let p = Point2D::new;
    Polygon::new(
        ClosedRing::new(vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn focus_lands_on_the_part_under_every_mirror_with_non_square_pixels() {
    // A 20 x 12 mm plate of 100 x 120 pixels: 0.2 x 0.1 mm pixels.
    let schedule = LayerSchedule::uniform(0.05, 0.1, 2).unwrap();
    let stack = ContourStack::new_unchecked(vec![
        ContourLayer::new(0.05, vec![square_mm(2.0, 1.0, 4.0, 3.0)]),
        ContourLayer::new(0.125, vec![square_mm(2.0, 1.0, 4.0, 3.0)]),
    ]);
    for (mx, my, x0, y0) in [
        (false, false, 60, 70),
        (true, false, 30, 70),
        (false, true, 60, 30),
        (true, true, 30, 30),
    ] {
        let map = PixelMap::new(
            mm(20.0),
            mm(12.0),
            PixelResolution::new(100, 120).unwrap(),
            Mirror { x: mx, y: my },
        );
        let input = ReviewInput::new(
            vec![ReviewLeaf::new(None, &stack)],
            &schedule,
            map,
            mm(5.0),
        )
        .unwrap();
        assert_eq!(input.pixel_size_mm(), [0.2, 0.1]);
        let regions = PlateRegions::compute(&input, &RAW);
        let region = regions.layer(1).solid[0];
        assert_eq!(region.area_px, 10 * 20, "mirror {mx} {my}");
        assert_eq!(
            region.bbox,
            PixelBox {
                x0,
                y0,
                x1: x0 + 10,
                y1: y0 + 20
            },
            "mirror {mx} {my}"
        );
        let focus = input.focus(1, region.centroid_px);
        assert!(
            (focus.x_mm - 3.0).abs() < 1e-9,
            "mirror {mx} {my}: {focus:?}"
        );
        assert!(
            (focus.y_mm - 2.0).abs() < 1e-9,
            "mirror {mx} {my}: {focus:?}"
        );
        assert_eq!(focus.z_mm, 0.125);
        let inside = input.pixel_to_plate(region.inside_point());
        assert!((2.0..4.0).contains(&inside[0]) && (1.0..3.0).contains(&inside[1]));
        let corner = input.pixel_to_plate([f64::from(region.bbox.x0), f64::from(region.bbox.y0)]);
        let far = input.pixel_to_plate([f64::from(region.bbox.x1), f64::from(region.bbox.y1)]);
        let (lo_x, hi_x) = (corner[0].min(far[0]), corner[0].max(far[0]));
        let (lo_y, hi_y) = (corner[1].min(far[1]), corner[1].max(far[1]));
        assert!((lo_x - 2.0).abs() < 1e-9 && (hi_x - 4.0).abs() < 1e-9);
        assert!((lo_y - 1.0).abs() < 1e-9 && (hi_y - 3.0).abs() < 1e-9);
    }
}

#[test]
fn a_cup_open_at_the_last_layer_never_touches_the_screen_edge() {
    // A cup: a floor on layer 0 and walls on layers 1-3, the last layer, so
    // nothing closes it above. Its air never reaches the screen edge, yet it
    // is open towards the resin above the last layer: a check must not call
    // it sealed from `touches_border` alone.
    let regions = FixtureBuilder::new(4)
        .model(0..1, Shape::rect(10.0, 10.0, 30.0, 30.0))
        .model(1..4, Shape::frame(10.0, 10.0, 30.0, 30.0, 2.0))
        .build()
        .regions();
    let enclosed = |r: &RegionRef| !regions.layer(r.layer).air[r.index as usize].touches_border;
    let air = regions.group_air(|_, _| true);
    let cups: Vec<&[RegionRef]> = air
        .iter()
        .filter(|members| members.iter().any(enclosed))
        .collect();
    assert_eq!(cups.len(), 1);
    assert!(cups[0].iter().all(enclosed));
    assert_eq!(
        cups[0].last().map(|r| r.layer),
        Some(regions.layers().len() - 1)
    );
}

#[test]
fn layer_spans_tile_the_height_from_the_plate() {
    // First layer 0.1 mm, then 0.05 mm layers.
    let schedule = LayerSchedule::uniform(0.05, 0.1, 4).unwrap();
    let map = PixelMap::new(
        mm(10.0),
        mm(10.0),
        PixelResolution::new(10, 10).unwrap(),
        Mirror::default(),
    );
    let input = ReviewInput::new(Vec::new(), &schedule, map, mm(5.0)).unwrap();
    let spans = input.layers();
    assert_eq!(spans[0].bottom_mm, 0.0);
    for pair in spans.windows(2) {
        assert_eq!(pair[0].top_mm, pair[1].bottom_mm);
    }
    for span in spans {
        assert!(span.bottom_mm < span.sample_mm && span.sample_mm < span.top_mm);
        assert!((span.sample_mm - 0.5 * (span.bottom_mm + span.top_mm)).abs() < 1e-12);
    }
    assert!((spans[3].top_mm - 0.25).abs() < 1e-12);
}
