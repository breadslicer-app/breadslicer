/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Thin-feature tests (#92).
//!
//! The fake printer has 0.1 mm pixels and 0.05 mm layers, so a feature is
//! thin under 0.2 mm, an error under 0.1 mm, and pieces of consecutive
//! layers join within a pixel of each other. Shapes are in pixels.

use sc_print_primitives::{Mirror, PixelResolution, PositiveFiniteMm};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::LayerSchedule;

use super::*;
use crate::input::ReviewLeaf;
use crate::regions::PlateRegions;
use crate::test_support::{Fixture, FixtureBuilder, Shape};

fn thin(fixture: &Fixture) -> Vec<Finding> {
    thin_with(fixture, &ThinFeatureOptions::default())
}

fn thin_with(fixture: &Fixture, options: &ThinFeatureOptions) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, options))
}

/// A strip `thickness` pixels across, along the segment from `a` to `b`.
fn strip(a: [f64; 2], b: [f64; 2], thickness: f64) -> Shape {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let scale = 0.5 * thickness / dx.hypot(dy);
    let (nx, ny) = (-dy * scale, dx * scale);
    Shape::polygon(
        vec![
            [a[0] + nx, a[1] + ny],
            [b[0] + nx, b[1] + ny],
            [b[0] - nx, b[1] - ny],
            [a[0] - nx, a[1] - ny],
        ],
        Vec::new(),
    )
}

/// A triangle with its apex at `apex`, opening to the right by `angle_deg`
/// and `length` pixels long.
fn wedge(apex: [f64; 2], angle_deg: f64, length: f64) -> Shape {
    let half = angle_deg.to_radians() / 2.0;
    let reach = length * half.tan();
    Shape::polygon(
        vec![
            apex,
            [apex[0] + length, apex[1] - reach],
            [apex[0] + length, apex[1] + reach],
        ],
        Vec::new(),
    )
}

fn layers(finding: &Finding) -> (usize, usize) {
    (finding.first_layer, finding.last_layer)
}

/// Whether the finding's focus lies in the pixel box `x0..x1` by `y0..y1`.
fn focus_in(fixture: &Fixture, finding: &Finding, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    let [ax, ay] = fixture.pixel_to_plate([x0, y0]);
    let [bx, by] = fixture.pixel_to_plate([x1, y1]);
    let (x, y) = (finding.focus.x_mm, finding.focus.y_mm);
    (ax.min(bx)..=ax.max(bx)).contains(&x) && (ay.min(by)..=ay.max(by)).contains(&y)
}

#[test]
fn a_wide_block_has_no_thin_feature() {
    // 40 x 30 pixels (4 x 3 mm) on every layer of an 8-layer plate.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = fixture.check(|ctx| run(ctx, &ThinFeatureOptions::default()));
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_thin_wall_is_one_warning_over_its_layers() {
    // A wall 1.5 pixels thick and 60 long stands on the plate on layers 0
    // to 11, next to a wide block.
    let fixture = FixtureBuilder::new(16)
        .model(0..12, Shape::rect(100.0, 20.0, 101.5, 80.0))
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let wall = &findings[0];
    assert_eq!(wall.check, CheckKind::ThinFeature);
    assert_eq!(wall.severity, Severity::Warning);
    assert_eq!(layers(wall), (0, 11));
    assert_eq!(wall.scope, RoleScope::Model);
    assert_eq!(wall.title, "Wall thinner than 2 pixels");
    assert_eq!(
        wall.detail,
        "This wall is 0.150 mm (1.5 px) wide, under 2 pixels: it will not print."
    );
    assert_eq!(wall.fix, None);
    assert!(
        focus_in(&fixture, wall, 100.0, 20.0, 101.5, 80.0),
        "{wall:?}"
    );
    let z = wall.focus.z_mm;
    assert!(
        fixture.input().layers()[..12]
            .iter()
            .any(|l| l.sample_mm == z)
    );
}

#[test]
fn walls_are_caught_across_x_across_y_and_on_the_diagonal() {
    let plate = |thickness: f64| {
        FixtureBuilder::new(10)
            // Thin across X, across Y, and at 45 and 30 degrees.
            .model(.., Shape::rect(20.0, 10.0, 20.0 + thickness, 50.0))
            .model(.., Shape::rect(40.0, 20.0, 90.0, 20.0 + thickness))
            .model(.., strip([110.0, 10.0], [150.0, 50.0], thickness))
            .model(.., strip([40.0, 70.0], [100.0, 105.0], thickness))
            .build()
    };
    let findings = thin(&plate(1.25));
    assert_eq!(findings.len(), 4, "{findings:?}");
    for wall in &findings {
        assert_eq!(wall.severity, Severity::Warning, "{wall:?}");
        assert_eq!(layers(wall), (0, 9), "{wall:?}");
        assert_eq!(wall.title, "Wall thinner than 2 pixels", "{wall:?}");
        // Measured to a quarter pixel, whatever the direction.
        assert!(
            ["(1 px)", "(1.25 px)", "(1.5 px)"]
                .iter()
                .any(|w| wall.detail.contains(w)),
            "{wall:?}"
        );
    }
    // The same walls 2.5 pixels thick print.
    let findings = thin(&plate(2.5));
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_wall_under_a_pixel_is_an_error() {
    let fixture = FixtureBuilder::new(12)
        .model(.., Shape::rect(100.0, 20.0, 100.5, 80.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let wall = &findings[0];
    assert_eq!(wall.severity, Severity::Error);
    assert_eq!(layers(wall), (0, 11));
    assert_eq!(wall.title, "Wall thinner than 1 pixel");
    assert_eq!(
        wall.detail,
        "This wall is 0.050 mm (0.5 px) wide, under 1 pixel: it will not print."
    );
}

#[test]
fn a_pin_under_a_pixel_is_caught_although_the_printer_drops_it() {
    // A pin 0.6 pixels across, centred on a pixel corner: no pixel centre
    // falls inside it, so the printer's own raster has nothing there.
    let fixture = FixtureBuilder::new(30)
        .model(.., Shape::circle(100.0, 60.0, 0.3))
        .build();
    assert!(fixture.regions().layer(10).is_empty());
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let pin = &findings[0];
    assert_eq!(pin.severity, Severity::Error);
    assert_eq!(layers(pin), (0, 29));
    assert_eq!(pin.title, "Pin thinner than 1 pixel");
    assert_eq!(
        pin.detail,
        "This pin is 0.050 mm (0.5 px) wide, under 1 pixel: it will not print."
    );
    assert!(focus_in(&fixture, pin, 99.5, 59.5, 100.5, 60.5), "{pin:?}");
}

#[test]
fn a_pin_between_one_and_two_pixels_is_a_warning() {
    let fixture = FixtureBuilder::new(20)
        .model(.., Shape::circle(100.0, 60.0, 0.75))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let pin = &findings[0];
    assert_eq!(pin.severity, Severity::Warning);
    assert_eq!(layers(pin), (0, 19));
    assert_eq!(pin.title, "Pin thinner than 2 pixels");
    assert_eq!(
        pin.detail,
        "This pin is 0.150 mm (1.5 px) wide, under 2 pixels: it will not print."
    );
}

#[test]
fn a_thin_wall_on_a_thick_base_starts_where_the_wall_starts() {
    // A block on layers 0 to 9 and a 1-pixel wall on top of it, layers 10
    // to 19.
    let fixture = FixtureBuilder::new(24)
        .model(0..10, Shape::rect(40.0, 20.0, 120.0, 80.0))
        .model(10..20, Shape::rect(60.0, 30.0, 61.0, 70.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(layers(&findings[0]), (10, 19));
    assert_eq!(findings[0].severity, Severity::Warning);
}

#[test]
fn a_leaning_wall_is_one_feature() {
    // A 1.5-pixel wall moving 2 pixels sideways per layer: consecutive
    // layers do not overlap, but stay within a pixel of each other.
    let mut builder = FixtureBuilder::new(10);
    for k in 0..10 {
        let x = 60.0 + 2.0 * k as f64;
        builder = builder.model(k..=k, Shape::rect(x, 20.0, x + 1.5, 80.0));
    }
    let findings = thin(&builder.build());
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(layers(&findings[0]), (0, 9));

    // 3 pixels per layer leaves 1.5 pixels between them: ten separate
    // slivers, each with nothing above or below it. Layer 0 rests on the
    // plate.
    let mut builder = FixtureBuilder::new(10);
    for k in 0..10 {
        let x = 60.0 + 3.0 * k as f64;
        builder = builder.model(k..=k, Shape::rect(x, 20.0, x + 1.5, 80.0));
    }
    let findings = thin(&builder.build());
    let spans: Vec<(usize, usize)> = findings.iter().map(layers).collect();
    assert_eq!(spans, (1..10).map(|k| (k, k)).collect::<Vec<_>>());
}

#[test]
fn ridges_keels_and_cone_tips_are_surface_steps() {
    let mut builder = FixtureBuilder::new(24);
    for k in 0..=10 {
        let inset = 2.0 * k as f64;
        // A ridge narrowing to a 1-pixel crest on layer 10, on 5 pixels.
        builder = builder.model(k..=k, Shape::rect(10.0 + inset, 10.0, 51.0 - inset, 50.0));
        // A keel hanging from layer 23 down to a 1-pixel sliver on layer
        // 13, held by the wide layers above it.
        builder = builder.model(
            23 - k..=23 - k,
            Shape::rect(70.0 + inset, 10.0, 111.0 - inset, 50.0),
        );
    }
    // A cone whose radius shrinks by a quarter pixel per layer: its last
    // three layers are thinner than 2 pixels, on top of each other.
    for k in 0..8 {
        let r = 2.0 - 0.25 * k as f64;
        builder = builder.model(k..=k, Shape::circle(150.0, 80.0, r));
    }
    let findings = thin(&builder.build());
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn corners_and_curves_stay_quiet() {
    let fixture = FixtureBuilder::new(12)
        // Corners of 90, 60 and 45 degrees.
        .model(.., Shape::rect(10.0, 10.0, 30.0, 30.0))
        .model(.., wedge([40.0, 20.0], 60.0, 20.0))
        .model(.., wedge([70.3, 20.2], 45.0, 20.0))
        .model(
            ..,
            Shape::polygon(
                vec![[110.0, 5.0], [125.0, 20.0], [110.0, 35.0], [95.0, 20.0]],
                Vec::new(),
            ),
        )
        // Walls 2.5 pixels thick, straight and round, and a pin 3 across.
        .model(.., Shape::frame(10.0, 50.0, 50.0, 90.0, 2.5))
        .model(.., Shape::ring(90.0, 70.0, 15.0, 12.5))
        .model(.., Shape::circle(150.0, 70.0, 1.5))
        .build();
    let findings = thin(&fixture);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_sharp_edge_is_reported_as_an_edge() {
    // A 10-degree blade edge standing over 12 layers: its last pixels
    // before it reaches 2 pixels thick will not print.
    let fixture = FixtureBuilder::new(12)
        .model(.., wedge([40.0, 60.0], 10.0, 60.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let edge = &findings[0];
    assert_eq!(edge.title, "Edge thinner than 2 pixels");
    assert_eq!(edge.severity, Severity::Warning);
    assert_eq!(layers(edge), (0, 11));
    assert!(focus_in(&fixture, edge, 40.0, 58.0, 60.0, 62.0), "{edge:?}");
}

#[test]
fn a_thin_neck_between_two_blocks_is_flagged() {
    // Two blocks joined by a 1-pixel strap 8 pixels long.
    let fixture = FixtureBuilder::new(10)
        .model(.., Shape::rect(20.0, 20.0, 50.0, 60.0))
        .model(.., Shape::rect(50.0, 39.5, 58.0, 40.5))
        .model(.., Shape::rect(58.0, 20.0, 90.0, 60.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let neck = &findings[0];
    assert_eq!(layers(neck), (0, 9));
    assert_eq!(neck.severity, Severity::Warning);
    assert_eq!(neck.title, "Wall thinner than 2 pixels");
    assert!(neck.detail.contains("(1 px)"), "{neck:?}");
    assert!(focus_in(&fixture, neck, 50.0, 39.0, 58.0, 41.0), "{neck:?}");
}

#[test]
fn a_thin_frame_is_one_wall() {
    // A square frame with walls 1.25 pixels thick.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::frame(40.0, 30.0, 80.0, 70.0, 1.25))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].title, "Wall thinner than 2 pixels");
    assert!(
        findings[0].detail.contains("(1.25 px)"),
        "{:?}",
        findings[0]
    );
}

#[test]
fn tiny_specks_are_dropped_and_a_flat_strand_is_kept() {
    // A 1 x 1 pixel speck alone on layer 5, and a strand 1 pixel thick and
    // 30 long lying flat on layer 8, with nothing above or below either.
    let fixture = FixtureBuilder::new(12)
        .model(5..=5, Shape::rect(40.0, 40.0, 41.0, 41.0))
        .model(8..=8, Shape::rect(100.0, 40.0, 130.0, 41.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(layers(&findings[0]), (8, 8));
    assert!(focus_in(&fixture, &findings[0], 100.0, 40.0, 130.0, 41.0));
}

#[test]
fn leaves_of_one_role_are_measured_together() {
    // Two model leaves, each a 1.5-pixel wall, side by side: together a
    // 3-pixel wall, which prints.
    let fixture = FixtureBuilder::new(8)
        .shape("Left", Role::Model, .., Shape::rect(60.0, 20.0, 61.5, 80.0))
        .shape(
            "Right",
            Role::Model,
            ..,
            Shape::rect(61.5, 20.0, 63.0, 80.0),
        )
        .build();
    let findings = thin(&fixture);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn supports_are_reported_only_under_one_pixel() {
    let fixture = FixtureBuilder::new(20)
        // A support 1.5 pixels across: thin for a model, fine for a support.
        .support(.., Shape::circle(40.0, 40.0, 0.75))
        // A support 0.6 pixels across, which will not print.
        .support(.., Shape::circle(80.0, 40.0, 0.3))
        // A support as thin, but inside a model block: the model prints it.
        .support(.., Shape::circle(140.0, 40.0, 0.3))
        .model(.., Shape::rect(130.0, 30.0, 150.0, 50.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let support = &findings[0];
    assert_eq!(support.severity, Severity::Error);
    assert_eq!(support.scope, RoleScope::Support);
    assert_eq!(support.title, "Support thinner than 1 pixel");
    assert_eq!(layers(support), (0, 19));
    assert!(
        focus_in(&fixture, support, 79.0, 39.0, 81.0, 41.0),
        "{support:?}"
    );

    let skip = ThinFeatureOptions {
        support_width_px: 0.0,
        ..ThinFeatureOptions::default()
    };
    assert!(thin_with(&fixture, &skip).is_empty());
}

#[test]
fn the_limits_follow_the_options() {
    let fixture = FixtureBuilder::new(10)
        .model(.., Shape::rect(100.0, 20.0, 101.5, 80.0))
        .model(.., Shape::rect(20.0, 20.0, 22.5, 80.0))
        .build();
    let finer = ThinFeatureOptions {
        model_width_px: 1.0,
        ..ThinFeatureOptions::default()
    };
    assert!(thin_with(&fixture, &finer).is_empty());
    let coarser = ThinFeatureOptions {
        model_width_px: 3.0,
        error_width_px: 2.0,
        ..ThinFeatureOptions::default()
    };
    let findings = thin_with(&fixture, &coarser);
    let found: Vec<(Severity, &str)> = findings
        .iter()
        .map(|f| (f.severity, f.title.as_str()))
        .collect();
    assert_eq!(
        found,
        vec![
            (Severity::Error, "Wall thinner than 2 pixels"),
            (Severity::Warning, "Wall thinner than 3 pixels"),
        ]
    );
    let bigger = ThinFeatureOptions {
        min_area_px: 10_000.0,
        ..ThinFeatureOptions::default()
    };
    assert!(thin_with(&fixture, &bigger).is_empty());
}

#[test]
fn a_mirrored_screen_reports_the_same_wall_where_it_is() {
    let build = |mirror: Mirror| {
        FixtureBuilder::new(8)
            .mirror(mirror)
            .model(.., Shape::rect(150.0, 20.0, 151.0, 50.0))
            .build()
    };
    for mirror in [
        Mirror { x: true, y: false },
        Mirror { x: false, y: true },
        Mirror { x: true, y: true },
    ] {
        let fixture = build(mirror);
        let findings = thin(&fixture);
        assert_eq!(findings.len(), 1, "{mirror:?}: {findings:?}");
        assert_eq!(layers(&findings[0]), (0, 7));
        assert!(
            focus_in(&fixture, &findings[0], 150.0, 20.0, 151.0, 50.0),
            "{mirror:?}: {findings:?}"
        );
    }
}

fn mm(v: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(v).unwrap()
}

/// A rectangle in plate millimetres.
fn rect_mm(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
    let p = Point2D::new;
    Polygon::new(
        ClosedRing::new(vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn non_square_pixels_are_counted_in_pixels_and_reported_in_millimetres() {
    // A 20 x 12 mm plate of 100 x 120 pixels: 0.2 x 0.1 mm pixels.
    // Wall A is 0.3 mm thick along X (1.5 pixels): thin.
    // Wall B is 0.3 mm thick along Y (3 pixels): fine.
    // Wall C is 0.15 mm thick along Y (1.5 pixels): thin.
    let schedule = LayerSchedule::uniform(0.05, 0.05, 6).unwrap();
    let layer = |z: f64| {
        ContourLayer::new(
            z,
            vec![
                rect_mm(-8.0, -4.0, -7.7, 4.0),
                rect_mm(-4.0, -4.0, 4.0, -3.7),
                rect_mm(-4.0, 2.0, 4.0, 2.15),
            ],
        )
    };
    let stack = ContourStack::new_unchecked(schedule.iter_z().map(layer).collect());
    let map = PixelMap::new(
        mm(20.0),
        mm(12.0),
        PixelResolution::new(100, 120).unwrap(),
        Mirror::default(),
    );
    let input = ReviewInput::new(
        vec![ReviewLeaf::new(None, Role::Model, &stack)],
        &schedule,
        map,
        mm(5.0),
    )
    .unwrap();
    let regions = PlateRegions::compute(&input);
    let mut findings = run(
        &ReviewContext::new(&input, &regions),
        &ThinFeatureOptions::default(),
    );
    findings.sort_by(|a, b| a.focus.x_mm.total_cmp(&b.focus.x_mm));
    let details: Vec<&str> = findings.iter().map(|f| f.detail.as_str()).collect();
    assert_eq!(
        details,
        vec![
            "This wall is 0.300 mm (1.5 px) wide, under 2 pixels: it will not print.",
            "This wall is 0.150 mm (1.5 px) wide, under 2 pixels: it will not print.",
        ]
    );
    assert!(findings[0].focus.x_mm < -7.6 && findings[1].focus.y_mm > 1.9);
}

#[test]
fn the_runner_lists_thin_features_and_runs_are_repeatable() {
    let fixture = FixtureBuilder::new(10)
        .model(.., Shape::rect(100.0, 20.0, 101.5, 80.0))
        .model(.., strip([20.0, 20.0], [60.0, 80.0], 0.75))
        .build();
    let first = thin(&fixture);
    assert_eq!(first, thin(&fixture));
    let review = fixture.review();
    let listed: Vec<&Finding> = review
        .iter()
        .filter(|f| f.check == CheckKind::ThinFeature)
        .collect();
    assert_eq!(listed.len(), 2, "{review:?}");
    assert!(first.iter().all(|f| listed.contains(&f)));
}

#[test]
fn morphology_on_runs_matches_a_bitmap() {
    // An irregular shape, touching the grid's edges, opened with every disc
    // up to 8 sub-pixels, against the same opening done pixel by pixel.
    let (w, h) = (40u32, 30u32);
    let lit = |x: u32, y: u32| {
        let (fx, fy) = (f64::from(x), f64::from(y));
        ((fx - 14.0).powi(2) + (fy - 12.0).powi(2) < 80.0)
            || (y == 20 && (3..36).contains(&x))
            || ((x + y).is_multiple_of(7) && x > 20)
            || (x.abs_diff(y + 5) <= 1 && y < 28)
            || x >= 35
            || (y <= 2 && x < 9)
    };
    let mut runs = Vec::new();
    for y in 0..h {
        let mut x = 0;
        while x < w {
            if lit(x, y) {
                let start = x;
                while x < w && lit(x, y) {
                    x += 1;
                }
                runs.push(Run::new(y, start, x));
            } else {
                x += 1;
            }
        }
    }
    let grid = FineGrid {
        map: PixelMap::new(
            mm(4.0),
            mm(3.0),
            PixelResolution::new(w, h).unwrap(),
            Mirror::default(),
        ),
        scale: 1,
        width: w,
        height: h,
    };
    for d in 1..=8 {
        let kernel = Kernel::disc(d);
        let fits = |ax: i64, ay: i64| {
            kernel.rows.iter().all(|&(dy, l, r)| {
                (l..=r).all(|dx| {
                    let (x, y) = (ax + dx, ay + dy);
                    x >= 0
                        && y >= 0
                        && x < i64::from(w)
                        && y < i64::from(h)
                        && lit(x as u32, y as u32)
                })
            })
        };
        let mut expected = Vec::new();
        for y in 0..i64::from(h) {
            for x in 0..i64::from(w) {
                let covered = kernel
                    .rows
                    .iter()
                    .any(|&(dy, l, r)| (l..=r).any(|dx| fits(x - dx, y - dy)));
                if covered {
                    expected.push((x, y));
                }
            }
        }
        let opened = open(&runs, &kernel, &grid);
        let mut got = Vec::new();
        for run in &opened {
            for x in run.x0..run.x1 {
                got.push((i64::from(x), i64::from(run.row)));
            }
        }
        got.sort_by_key(|&(x, y)| (y, x));
        assert_eq!(got, expected, "disc {d}");
        // And the thin part is the rest.
        let thin = difference(&runs, &opened);
        assert_eq!(
            runs::area_px(&thin) + runs::area_px(&opened),
            runs::area_px(&runs)
        );
    }
}

#[test]
fn discs_are_round_and_full_across_their_middle() {
    let widths = |d: u32| -> Vec<i64> {
        Kernel::disc(d)
            .rows
            .iter()
            .map(|&(_, l, r)| r - l + 1)
            .collect()
    };
    assert_eq!(widths(1), vec![1]);
    assert_eq!(widths(2), vec![2, 2]);
    assert_eq!(widths(4), vec![2, 4, 4, 2]);
    assert_eq!(widths(7), vec![3, 5, 7, 7, 7, 5, 3]);
    assert_eq!(widths(8), vec![4, 6, 8, 8, 8, 8, 6, 4]);
    for d in 1..=12 {
        let disc = Kernel::disc(d);
        assert!(
            disc.rows
                .iter()
                .any(|&(dy, l, r)| dy == 0 && l <= 0 && r >= 0)
        );
    }
}
