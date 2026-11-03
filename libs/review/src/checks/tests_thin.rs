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
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::LayerSchedule;
use sc_sl1cer::raster::pixel_map::PixelMap;

use super::lines::{self, Axis, Disc, Edges, Lines};
use super::*;
use crate::input::{ReviewLeaf, pixel_to_plate};
use crate::regions::PlateRegions;
use crate::runner::ReviewContext;
use crate::test_support::{Fixture, FixtureBuilder, Shape};

fn thin(fixture: &Fixture) -> Vec<Finding> {
    thin_with(fixture, &ThinFeatureOptions::default())
}

fn thin_with(fixture: &Fixture, options: &ThinFeatureOptions) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, options))
}

/// A strip `thickness` pixels across, along the segment from `a` to `b`.
fn strip(a: [f64; 2], b: [f64; 2], thickness: f64) -> Shape {
    Shape::polygon(strip_corners(a, b, thickness).to_vec(), Vec::new())
}

/// The corners of [`strip`].
fn strip_corners(a: [f64; 2], b: [f64; 2], thickness: f64) -> [[f64; 2]; 4] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let scale = 0.5 * thickness / dx.hypot(dy);
    let (nx, ny) = (-dy * scale, dx * scale);
    [
        [a[0] + nx, a[1] + ny],
        [b[0] + nx, b[1] + ny],
        [b[0] - nx, b[1] - ny],
        [a[0] - nx, a[1] - ny],
    ]
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
    assert_eq!(
        wall.fix,
        Some(SuggestedFix::Thicken {
            at: wall.focus,
            feature: 0
        })
    );
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
    // #409: the highlight holds the wall's pixels, not the block beside it.
    let footprint = wall.footprint.as_ref().expect("a footprint");
    assert!(!footprint.slabs.is_empty());
    let [ax, ay] = fixture.pixel_to_plate([100.0, 20.0]);
    let [bx, by] = fixture.pixel_to_plate([101.5, 80.0]);
    let (x0, x1, y0, y1) = (ax.min(bx), ax.max(bx), ay.min(by), ay.max(by));
    let pixel_mm = 0.1;
    for slab in &footprint.slabs {
        assert!(slab.bottom_mm < 12.0 * 0.05, "{slab:?}");
        assert!(!slab.rects.is_empty());
        for r in &slab.rects {
            assert!(
                r.x0_mm >= x0 - pixel_mm
                    && r.x1_mm <= x1 + pixel_mm
                    && r.y0_mm >= y0 - pixel_mm
                    && r.y1_mm <= y1 + pixel_mm,
                "{r:?}"
            );
        }
    }
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
        // Measured to a few hundredths of a pixel, whatever the direction.
        assert!((reported_px(wall) - 1.25).abs() <= 0.03, "{wall:?}");
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
        "This pin is 0.060 mm (0.6 px) wide, under 1 pixel: it will not print."
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
    // three layers are thinner than 2 pixels, on top of each other. (A
    // hundredth of a pixel wider, so that the 48-gon before them, of radius
    // 1, is not 1.996 pixels across, under 2.)
    for k in 0..8 {
        let r = 2.01 - 0.25 * k as f64;
        builder = builder.model(k..=k, Shape::circle(150.0, 80.0, r));
    }
    let findings = thin(&builder.build());
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_slender_tip_thin_over_several_layers_is_reported() {
    // A spike on a thick base narrowing by 0.1 pixel a layer, from 3 pixels
    // on layer 5 to 0.2 on layer 33: under 2 pixels from layer 15 (whose
    // 48-gon is 1.996 pixels across). Its last two layers, 0.3 pixels and
    // less around a pixel corner, cover no sub-pixel centre and are not seen.
    let mut builder = FixtureBuilder::new(40).model(0..5, Shape::rect(80.0, 40.0, 120.0, 80.0));
    for k in 5..34 {
        let d = 3.0 - 0.1 * (k - 5) as f64;
        builder = builder.model(k..=k, Shape::circle(100.0, 60.0, d / 2.0));
    }
    let findings = thin(&builder.build());
    assert_eq!(findings.len(), 1, "{findings:?}");
    let tip = &findings[0];
    assert_eq!(tip.title, "Pin thinner than 2 pixels");
    assert_eq!(layers(tip), (15, 31));
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
        min_size: 10_000.0,
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

/// Lines `0..count`, each with the stretches `stretches(line)`.
fn lines_of(count: i64, stretches: impl Fn(i64) -> Vec<(f64, f64)>) -> Lines {
    let mut lines = Lines::starting_at(0);
    for i in 0..count {
        lines.push_line(&stretches(i));
    }
    lines
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

#[test]
fn opening_a_rectangle_rounds_its_corners_exactly() {
    // Lines 5 to 24 hold [10, 40]; a disc of radius 4 on lines 1 apart.
    let rect = lines_of(30, |i| {
        if (5..25).contains(&i) {
            vec![(10.0, 40.0)]
        } else {
            Vec::new()
        }
    });
    let opened = lines::open(&rect, &Disc::new(4.0, 1.0));
    for i in 0..30 {
        let got = opened.line(i);
        // The centres fit on lines 9 to 20, between 14 and 36; a line d
        // away from the nearest of them gets the disc's half-width there.
        let d: i64 = if i < 9 {
            9 - i
        } else if i > 20 {
            i - 20
        } else {
            0
        };
        if !(5..25).contains(&i) {
            assert!(got.is_empty(), "line {i}: {got:?}");
            continue;
        }
        let half = (16.0 - (d * d) as f64).sqrt();
        assert_eq!(got.len(), 1, "line {i}: {got:?}");
        assert!(
            close(got[0].0, 14.0 - half) && close(got[0].1, 36.0 + half),
            "line {i}: {got:?}, half-width {half}"
        );
    }
}

#[test]
fn a_disc_fits_a_wall_exactly_as_wide_and_no_wider() {
    let wall = lines_of(40, |_| vec![(10.0, 17.3)]);
    let opened = lines::open(&wall, &Disc::new(3.65, 1.0));
    let middle = opened.line(20);
    assert!(
        middle.len() == 1 && close(middle[0].0, 10.0) && close(middle[0].1, 17.3),
        "{middle:?}"
    );
    let opened = lines::open(&wall, &Disc::new(3.65 + 1e-6, 1.0));
    assert!(opened.line(20).is_empty());
    // Across the lines, a disc falling between them reaches one line more.
    assert_eq!(Disc::new(4.0, 1.0).reach(), 4);
    assert_eq!(Disc::new(4.2, 1.0).reach(), 5);
}

#[test]
fn opening_stays_inside_the_solid_and_is_idempotent() {
    let solid = lines_of(60, |i| {
        let y = i as f64;
        let mut out = vec![(10.0 + 3.0 * (y / 3.0).sin(), 30.0 + 5.0 * (y / 5.0).cos())];
        if i % 9 != 4 {
            out.push((35.0, 35.0 + (i % 7) as f64 * 1.7));
        }
        if (20..44).contains(&i) {
            out.push((48.0 + 0.4 * y, 53.5 + 0.4 * y));
        }
        out.retain(|&(a, b)| b > a);
        out
    });
    for radius in [0.5, 1.5, 2.0, 3.7] {
        let disc = Disc::new(radius, 1.0);
        let opened = lines::open(&solid, &disc);
        let again = lines::open(&opened, &disc);
        for i in -10..70 {
            for &(a, b) in opened.line(i) {
                assert!(
                    solid
                        .line(i)
                        .iter()
                        .any(|&(x0, x1)| a >= x0 - 1e-9 && b <= x1 + 1e-9),
                    "radius {radius}, line {i}: {a}..{b} outside {:?}",
                    solid.line(i)
                );
            }
            let (p, q) = (opened.line(i), again.line(i));
            assert_eq!(p.len(), q.len(), "radius {radius}, line {i}");
            for (s, t) in p.iter().zip(q) {
                assert!(
                    (s.0 - t.0).abs() < 1e-6 && (s.1 - t.1).abs() < 1e-6,
                    "radius {radius}, line {i}: {p:?} then {q:?}"
                );
            }
        }
    }
}

#[test]
fn cutting_finds_the_exact_crossings_on_rows_and_columns() {
    // A right triangle (10, 10), (50, 10), (10, 50) in pixels, and two
    // leaves side by side that must cut as one stretch.
    let fixture = FixtureBuilder::new(1)
        .model(
            ..,
            Shape::polygon(vec![[10.0, 10.0], [50.0, 10.0], [10.0, 50.0]], Vec::new()),
        )
        .shape(
            "Left",
            Role::Model,
            ..,
            Shape::rect(100.0, 20.0, 101.5, 80.0),
        )
        .shape(
            "Right",
            Role::Model,
            ..,
            Shape::rect(101.5, 20.0, 103.0, 80.0),
        )
        .build();
    let input = fixture.input();
    let contours: Vec<&ContourLayer> = input
        .leaves()
        .iter()
        .map(|leaf| &leaf.stack.layers()[0])
        .collect();
    let size = [800, 480];
    let rows = Edges::new(&contours, input.pixel_map(), 4.0, Axis::Rows, size);
    let columns = Edges::new(&contours, input.pixel_map(), 4.0, Axis::Columns, size);
    let on_rows = rows.lines(0.5, 1.0, 0, 480);
    let on_columns = columns.lines(0.5, 1.0, 0, 800);
    // In sub-pixels the triangle is (40, 40), (200, 40), (40, 200).
    for k in [40_i64, 41, 100, 199] {
        let at = k as f64 + 0.5;
        for (lines, name) in [(&on_rows, "row"), (&on_columns, "column")] {
            let got = lines.line(k);
            assert!(
                close(got[0].0, 40.0) && close(got[0].1, 240.0 - at),
                "{name} {k}: {got:?}"
            );
        }
    }
    // The two leaves: one stretch from 400 to 412 on row 200.
    let row = on_rows.line(200);
    assert_eq!(row.len(), 1, "{row:?}");
    assert!(close(row[0].0, 400.0) && close(row[0].1, 412.0), "{row:?}");
    // The sub-pixels of a solid are those whose centres are in it.
    let cells = on_rows.cells(800, false);
    // Row 100 crosses the triangle at 139.5: the centre of sub-pixel 139
    // is on the end, and out, as in the printer's raster.
    let row_100: Vec<Run> = cells.iter().filter(|r| r.row == 100).copied().collect();
    assert_eq!(
        row_100,
        vec![Run::new(100, 40, 139), Run::new(100, 400, 412)]
    );
}

#[test]
fn a_line_cut_near_a_point_matches_the_whole_line() {
    // Holes, slanted edges, and two leaves overlapping (winding 2): a line
    // cut only between two points along it, from the edges near them, is
    // the whole line cut and clipped there.
    let fixture = FixtureBuilder::new(1)
        .model(.., Shape::ring(60.0, 60.0, 30.0, 12.0))
        .model(.., strip([10.0, 100.0], [190.0, 20.0], 3.3))
        .model(
            ..,
            Shape::polygon(
                vec![[120.0, 10.0], [190.0, 40.0], [130.0, 110.0]],
                Vec::new(),
            ),
        )
        .shape(
            "Other",
            Role::Model,
            ..,
            Shape::rect(40.0, 50.0, 150.0, 70.0),
        )
        .shape("Other", Role::Model, .., Shape::circle(170.0, 90.0, 0.4))
        .build();
    let input = fixture.input();
    let contours: Vec<&ContourLayer> = input
        .leaves()
        .iter()
        .map(|leaf| &leaf.stack.layers()[0])
        .collect();
    for axis in [Axis::Rows, Axis::Columns] {
        let edges = Edges::new(&contours, input.pixel_map(), 4.0, axis, [800, 480]);
        for k in 0..500 {
            let y = 0.93f64.mul_add(f64::from(k), 3.1);
            let whole = edges.lines(y, 1.0, 0, 1);
            for m in 0..40 {
                let lo = 21.7f64.mul_add(f64::from(m), -15.0);
                let hi = lo + 18.5;
                let want: Vec<(f64, f64)> = whole
                    .line(0)
                    .iter()
                    .map(|&(a, b)| (a.max(lo), b.min(hi)))
                    .filter(|&(a, b)| a < b)
                    .collect();
                let got = edges.cut_line(y, lo, hi);
                assert_eq!(got.len(), want.len(), "{axis:?} at {y}, {lo}..{hi}");
                for (g, w) in got.iter().zip(&want) {
                    assert!(
                        close(g.0, w.0) && close(g.1, w.1),
                        "{axis:?} at {y}, {lo}..{hi}: {got:?} against {want:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_local_measurement_is_exact_across_its_lines() {
    // A pin 1.8 pixels across off any grid phase, and a wall 1.3 pixels
    // thick across Y: measured on rows and columns 1/16 pixel apart.
    let fixture = FixtureBuilder::new(1)
        .model(.., Shape::circle(100.37, 60.21, 0.9))
        .model(.., Shape::rect(20.0, 30.1, 80.0, 31.4))
        .build();
    let input = fixture.input();
    let contours: Vec<&ContourLayer> = input
        .leaves()
        .iter()
        .map(|leaf| &leaf.stack.layers()[0])
        .collect();
    let size = [800, 480];
    let rows = Edges::new(&contours, input.pixel_map(), 4.0, Axis::Rows, size);
    let columns = Edges::new(&contours, input.pixel_map(), 4.0, Axis::Columns, size);
    let measure = Measure {
        rows: &rows,
        columns: &columns,
        step: 0.25,
        reach: 9.0,
        max: 8.0,
    };
    // The polygon's inscribed circle is a little smaller than the circle.
    let pin = 1.8 * (std::f64::consts::PI / 48.0).cos();
    for cell in [[401, 240], [400, 241], [402, 241], [401, 242]] {
        let got = measure.at(cell) / 4.0;
        assert!(
            got <= pin + 1e-6 && got >= pin - 1.0 / 16.0,
            "{cell:?}: {got}"
        );
    }
    for cell in [[200, 121], [300, 124], [319, 122]] {
        let got = measure.at(cell) / 4.0;
        assert!((got - 1.3).abs() < 1e-3, "{cell:?}: {got}");
    }
    // At the limit and over it, the measurement stops at the limit.
    let wide = FixtureBuilder::new(1)
        .model(.., Shape::rect(20.0, 30.0, 80.0, 33.0))
        .build();
    let input = wide.input();
    let contours = vec![&input.leaves()[0].stack.layers()[0]];
    let rows = Edges::new(&contours, input.pixel_map(), 4.0, Axis::Rows, size);
    let columns = Edges::new(&contours, input.pixel_map(), 4.0, Axis::Columns, size);
    let measure = Measure {
        rows: &rows,
        columns: &columns,
        step: 0.25,
        reach: 9.0,
        max: 8.0,
    };
    assert_eq!(measure.at([200, 125]), 8.0);
}

// ---------------------------------------------------------------------------
// Problems found in review, each reproduced before it was fixed.
// ---------------------------------------------------------------------------

/// The width in pixels a finding reports: the `1.25` of "... (1.25 px) wide".
fn reported_px(finding: &Finding) -> f64 {
    let open = finding.detail.find('(').expect("a width in pixels");
    let rest = &finding.detail[open + 1..];
    rest[..rest.find(" px)").expect("a width in pixels")]
        .parse()
        .expect("a number")
}

/// The width in millimetres a finding reports: the `0.150` of "is 0.150 mm".
fn reported_mm(finding: &Finding) -> f64 {
    let at = finding.detail.find(" is ").expect("a width in millimetres") + 4;
    let rest = &finding.detail[at..];
    rest[..rest.find(" mm").expect("a width in millimetres")]
        .parse()
        .expect("a number")
}

#[test]
fn features_just_under_a_pixel_are_errors() {
    // A wall 0.9 pixels thick, and a pin 0.9 pixels across centred on a
    // pixel corner, which the printer drops altogether.
    let wall = FixtureBuilder::new(12)
        .model(.., Shape::rect(100.05, 20.0, 100.95, 80.0))
        .build();
    let pin = FixtureBuilder::new(30)
        .model(.., Shape::circle(100.0, 60.0, 0.45))
        .build();
    assert!(pin.regions().layer(10).is_empty());
    for fixture in [&wall, &pin] {
        let findings = thin(fixture);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].severity, Severity::Error, "{findings:?}");
        assert!(
            (reported_px(&findings[0]) - 0.9).abs() <= 0.05,
            "{findings:?}"
        );
    }
}

#[test]
fn a_support_just_under_a_pixel_is_reported() {
    // A support 0.9 pixels across centred on a pixel corner: the printer
    // lights none of it.
    let fixture = FixtureBuilder::new(30)
        .support(.., Shape::circle(100.0, 60.0, 0.45))
        .build();
    assert!(fixture.regions().layer(10).is_empty());
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].scope, RoleScope::Support);
    assert_eq!(findings[0].severity, Severity::Error);
    // One 1.1 pixels across is over the support limit.
    let fixture = FixtureBuilder::new(30)
        .support(.., Shape::circle(100.0, 60.0, 0.55))
        .build();
    let findings = thin(&fixture);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn the_limits_do_not_depend_on_the_sub_pixel_phase() {
    // Walls across X, across Y and at 45 degrees, shifted by steps of 0.03
    // pixels over a quarter pixel: just under each limit they are caught at
    // the right severity, just over the 2-pixel limit they are not.
    for step in 0..9 {
        let o = 0.03 * f64::from(step);
        for (w, expected) in [
            (1.9, Some(Severity::Warning)),
            (1.95, Some(Severity::Warning)),
            (2.05, None),
            (2.1, None),
            (1.05, Some(Severity::Warning)),
            (1.1, Some(Severity::Warning)),
            (0.9, Some(Severity::Error)),
            (0.95, Some(Severity::Error)),
        ] {
            let fixture = FixtureBuilder::new(10)
                .model(.., Shape::rect(20.0 + o, 10.0, 20.0 + o + w, 50.0))
                .model(.., Shape::rect(40.0, 20.0 + o, 90.0, 20.0 + o + w))
                .model(.., strip([110.0 + o, 10.0], [150.0 + o, 50.0], w))
                .build();
            let findings = thin(&fixture);
            let got: Vec<Severity> = findings.iter().map(|f| f.severity).collect();
            let want: Vec<Severity> = expected.into_iter().cycle().take(3).collect();
            assert_eq!(got, want, "{w} px at +{o}: {findings:?}");
            for finding in &findings {
                assert!(
                    (reported_px(finding) - w).abs() <= 0.05,
                    "{w} px at +{o}: {finding:?}"
                );
            }
        }
    }
}

#[test]
fn round_pins_are_measured_closely_at_any_phase() {
    for (cx, cy) in [
        (100.0, 60.0),
        (100.125, 60.125),
        (100.5, 60.5),
        (100.375, 60.125),
        (100.25, 60.0),
    ] {
        let pin = |d: f64| {
            FixtureBuilder::new(20)
                .model(.., Shape::circle(cx, cy, d / 2.0))
                .build()
        };
        // A pin 2.2 pixels across prints.
        let findings = thin(&pin(2.2));
        assert!(findings.is_empty(), "2.2 px at ({cx}, {cy}): {findings:?}");
        for (d, severity) in [
            (1.8, Severity::Warning),
            (1.1, Severity::Warning),
            (0.9, Severity::Error),
        ] {
            let findings = thin(&pin(d));
            assert_eq!(findings.len(), 1, "{d} px at ({cx}, {cy}): {findings:?}");
            assert_eq!(findings[0].severity, severity, "{d} px at ({cx}, {cy})");
            assert!(
                (reported_px(&findings[0]) - d).abs() <= 0.1,
                "{d} px at ({cx}, {cy}): {findings:?}"
            );
        }
    }
}

#[test]
fn a_short_neck_holding_a_part_is_reported() {
    // A block on layers 10 to 20 held to a body standing on the plate only
    // by a neck 1 pixel wide and 1 to 3.5 pixels long.
    for len in [1.0, 1.5, 2.0, 2.5, 2.9, 3.0, 3.5] {
        let fixture = FixtureBuilder::new(21)
            .model(.., Shape::rect(20.0, 20.0, 50.0, 60.0))
            .model(10.., Shape::rect(50.0, 39.5, 50.0 + len, 40.5))
            .model(10.., Shape::rect(50.0 + len, 20.0, 90.0, 60.0))
            .build();
        let findings = thin(&fixture);
        assert_eq!(findings.len(), 1, "neck {len} px long: {findings:?}");
        assert_eq!(layers(&findings[0]), (10, 20), "neck {len} px long");
        assert!(
            focus_in(&fixture, &findings[0], 49.0, 39.0, 51.0 + len, 41.0),
            "neck {len} px long: {findings:?}"
        );
    }
    // Two blocks joined by a strap 1 x 2 pixels on every layer.
    let fixture = FixtureBuilder::new(20)
        .model(.., Shape::rect(20.0, 20.0, 50.0, 60.0))
        .model(.., Shape::rect(50.0, 39.5, 52.0, 40.5))
        .model(.., Shape::rect(52.0, 20.0, 90.0, 60.0))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].title, "Neck thinner than 2 pixels");
}

#[test]
fn bodies_touching_at_a_point_are_a_neck_under_a_pixel() {
    // Two blocks meeting only at the corner (50, 60) on every layer: the
    // printer joins them through one diagonal pair of pixels.
    let fixture = FixtureBuilder::new(20)
        .model(.., Shape::rect(20.0, 20.0, 50.0, 60.0))
        .model(.., Shape::rect(50.0, 60.0, 80.0, 100.0))
        .build();
    assert_eq!(fixture.regions().layer(5).solid.len(), 1);
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].title, "Neck thinner than 1 pixel");
    assert_eq!(findings[0].severity, Severity::Error);
    assert!(
        focus_in(&fixture, &findings[0], 48.0, 58.0, 52.0, 62.0),
        "{findings:?}"
    );
}

#[test]
fn supports_that_only_touch_are_not_reported() {
    // Two support shafts meeting at the corner (50, 60) on every layer, as
    // two shafts side by side whose facets graze (#156): each stands.
    let fixture = FixtureBuilder::new(20)
        .support(.., Shape::rect(20.0, 20.0, 50.0, 60.0))
        .support(.., Shape::rect(50.0, 60.0, 80.0, 100.0))
        .build();
    assert_eq!(fixture.regions().layer(5).solid.len(), 1);
    let findings = thin(&fixture);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn short_sub_pixel_spikes_are_reported_whatever_their_width() {
    // Free-standing spikes 0.25 to 1.5 pixels across, 6 layers tall (the
    // first rests on the plate): the thinnest are the worst, and need no
    // more layers than the others to be reported.
    // They are centred on a sub-pixel centre: a spike under about a third of
    // a pixel can fall between them and not be seen at all.
    for d in [0.25, 0.5, 0.75, 1.0, 1.5] {
        let fixture = FixtureBuilder::new(10)
            .model(0..6, Shape::circle(100.125, 60.125, d / 2.0))
            .build();
        let findings = thin(&fixture);
        assert_eq!(findings.len(), 1, "{d} px: {findings:?}");
        assert_eq!(layers(&findings[0]), (0, 5), "{d} px");
    }
    // The same at the real printer's ratio: 0.02 mm pixels, 0.1 mm layers,
    // a pin 0.01 mm square and 0.5 mm tall on a thick base.
    let fixture = FixtureBuilder::new(12)
        .screen(400, 300, 0.02)
        .layer_height_mm(0.1)
        .model(0..2, Shape::rect(100.0, 100.0, 300.0, 200.0))
        .model(2..7, Shape::rect(200.0, 150.0, 200.5, 150.5))
        .build();
    let findings = thin(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].severity, Severity::Error);
    assert_eq!(layers(&findings[0]), (2, 6));
}

#[test]
fn oblique_walls_on_non_square_pixels_report_their_true_thickness() {
    // 0.2 x 0.1 mm pixels. A wall 1.25 pixels thick at 45 degrees in pixel
    // space: in millimetres it is thinner than along either axis.
    let map = PixelMap::new(
        mm(20.0),
        mm(12.0),
        PixelResolution::new(100, 120).unwrap(),
        Mirror::default(),
    );
    let corners: Vec<[f64; 2]> = strip_corners([20.0, 20.0], [80.0, 80.0], 1.25)
        .iter()
        .map(|&p| pixel_to_plate(&map, p))
        .collect();
    // The true thickness: corner 0 to the line through corners 2 and 3.
    let (p, q, r) = (corners[0], corners[2], corners[3]);
    let (lx, ly) = (q[0] - r[0], q[1] - r[1]);
    let true_mm = ((p[0] - r[0]) * ly - (p[1] - r[1]) * lx).abs() / lx.hypot(ly);
    let mut ring = ClosedRing::new(corners.iter().map(|c| Point2D::new(c[0], c[1])).collect());
    if ring.signed_area() < 0.0 {
        ring.reverse();
    }
    let polygon = Polygon::new(ring, Vec::new()).unwrap();
    let schedule = LayerSchedule::uniform(0.05, 0.05, 6).unwrap();
    let stack = ContourStack::new_unchecked(
        schedule
            .iter_z()
            .map(|z| ContourLayer::new(z, vec![polygon.clone()]))
            .collect(),
    );
    let input = ReviewInput::new(
        vec![ReviewLeaf::new(None, Role::Model, &stack)],
        &schedule,
        map,
        mm(5.0),
    )
    .unwrap();
    let regions = PlateRegions::compute(&input);
    let findings = run(
        &ReviewContext::new(&input, &regions),
        &ThinFeatureOptions::default(),
    );
    assert_eq!(findings.len(), 1, "{findings:?}");
    let got = reported_mm(&findings[0]);
    assert!(
        (got - true_mm).abs() <= 0.03 * true_mm,
        "reported {got} mm, the wall is {true_mm:.4} mm: {findings:?}"
    );
}

#[test]
fn any_sub_pixel_setting_measures_the_same_features() {
    // A wall 0.5 pixels thick and a support 0.8 pixels across. A setting of
    // 1 is held to 2, the coarsest grid on which a feature under a pixel
    // still covers sub-pixel centres.
    let fixture = FixtureBuilder::new(20)
        .model(.., Shape::rect(100.0, 20.0, 100.5, 80.0))
        .support(.., Shape::circle(40.25, 40.25, 0.4))
        .build();
    for subpixels in [0, 1, 2, 3, 8, 40] {
        let options = ThinFeatureOptions {
            subpixels,
            ..ThinFeatureOptions::default()
        };
        let findings = thin_with(&fixture, &options);
        let found: Vec<(Severity, RoleScope)> =
            findings.iter().map(|f| (f.severity, f.scope)).collect();
        assert_eq!(
            found,
            vec![
                (Severity::Error, RoleScope::Support),
                (Severity::Error, RoleScope::Model),
            ],
            "{subpixels} sub-pixels: {findings:?}"
        );
    }
}

#[test]
fn eight_sub_pixels_fit_a_16k_screen() {
    // 15120 x 6230 pixels: eight times finer is 6e9 sub-pixels per layer,
    // more than a u32 run length.
    let fixture = FixtureBuilder::new(2)
        .screen(15120, 6230, 0.019)
        .model(.., Shape::rect(100.0, 100.0, 101.5, 400.0))
        .build();
    let options = ThinFeatureOptions {
        subpixels: 8,
        ..ThinFeatureOptions::default()
    };
    let findings = thin_with(&fixture, &options);
    assert_eq!(findings.len(), 1, "{findings:?}");
}

#[test]
fn a_dense_lattice_near_the_limit_is_decided_on_columns() {
    // A lattice of walls every 6 pixels: hundreds of segments per layer
    // between its crossings (where a 2-pixel disc fits), all near the limit,
    // too many to measure one by one. At 2.1 pixels it prints; at 1.9 it is
    // one thin feature.
    let lattice = |wall: f64| {
        let mut builder = FixtureBuilder::new(6);
        let mut x = 10.0;
        while x < 190.0 {
            builder = builder.model(.., Shape::rect(x, 10.0, x + wall, 110.0));
            x += 6.0;
        }
        let mut y = 10.0;
        while y < 110.0 {
            builder = builder.model(.., Shape::rect(10.0, y, 190.0, y + wall));
            y += 6.0;
        }
        builder.build()
    };
    let findings = thin(&lattice(2.1));
    assert!(findings.is_empty(), "{} findings", findings.len());
    let findings = thin(&lattice(1.9));
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].severity, Severity::Warning);
    assert!(
        (reported_px(&findings[0]) - 1.9).abs() <= 0.05,
        "{findings:?}"
    );
}

#[test]
fn batches_of_layers_give_the_same_findings_on_any_thread_count() {
    // 100 layers: a wall on all of them, a sub-pixel pin on 50 to 89, and a
    // wall leaning 2 pixels a layer on 30 to 44. One thread sweeps the plate
    // in batches of 4 layers.
    let mut builder = FixtureBuilder::new(100)
        .model(.., Shape::rect(100.0, 20.0, 101.5, 80.0))
        .model(50..90, Shape::circle(40.0, 40.0, 0.3));
    for k in 30..45 {
        let x = 130.0 + 2.0 * (k - 30) as f64;
        builder = builder.model(k..=k, Shape::rect(x, 90.0, x + 1.5, 110.0));
    }
    let fixture = builder.build();
    let on = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| thin(&fixture))
    };
    let all = thin(&fixture);
    assert_eq!(on(1), all);
    assert_eq!(on(3), all);
    let mut spans: Vec<(usize, usize)> = all.iter().map(layers).collect();
    spans.sort_unstable();
    assert_eq!(spans, vec![(0, 99), (30, 44), (50, 89)], "{all:?}");
}

#[test]
fn every_feature_is_measured_however_many_there_are() {
    // 400 free-standing pins on a large screen, each measured again before
    // it is reported: none keeps the coarse width of its sub-pixel phase.
    let field = |d: f64| {
        let mut builder = FixtureBuilder::new(10).screen(2000, 1200, 0.1);
        for i in 0..20 {
            for j in 0..20 {
                let (cx, cy) = (
                    50.0 + 90.0 * f64::from(i) + 0.07 * f64::from(j),
                    50.0 + 55.0 * f64::from(j) + 0.11 * f64::from(i),
                );
                builder = builder.model(.., Shape::circle(cx, cy, d / 2.0));
            }
        }
        builder.build()
    };
    for (d, severity, title) in [
        (1.1, Severity::Warning, "Pin thinner than 2 pixels"),
        (0.95, Severity::Error, "Pin thinner than 1 pixel"),
    ] {
        let findings = thin(&field(d));
        assert_eq!(findings.len(), 400, "{d} px");
        for finding in &findings {
            assert_eq!(finding.severity, severity, "{d} px: {finding:?}");
            assert_eq!(finding.title, title, "{d} px: {finding:?}");
            assert!(
                (reported_px(finding) - d).abs() <= 0.1 + 1e-9,
                "{d} px: {finding:?}"
            );
        }
    }
}

/// The thin findings of `fixture` with the thickenings they offer.
fn thickenings(fixture: &Fixture) -> (Vec<Finding>, Vec<CavityFill>) {
    let input = fixture.input();
    let regions = PlateRegions::compute(&input);
    run_with_thickenings(
        &ReviewContext::new(&input, &regions),
        &ThinFeatureOptions::default(),
    )
}

#[test]
fn a_thickening_grows_the_pixels_of_the_wall_by_one() {
    // A wall half a pixel thick inside pixel column 100, from row 20 to 80.
    let fixture = FixtureBuilder::new(12)
        .model(.., Shape::rect(100.25, 20.0, 100.75, 80.0))
        .build();
    let (findings, fills) = thickenings(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(fills.len(), 1);
    let fill = &fills[0];
    assert_eq!(fill.slabs.len(), 12);
    // Columns 99 to 101, rows 19 to 80: the wall's pixels and one more on
    // every side.
    let a = fixture.pixel_to_plate([99.0, 19.0]);
    let b = fixture.pixel_to_plate([102.0, 81.0]);
    for slab in &fill.slabs {
        assert_eq!(slab.rects.len(), 1, "{slab:?}");
        let rect = slab.rects[0];
        assert!((rect.x0_mm - a[0].min(b[0])).abs() < 1e-9, "{rect:?}");
        assert!((rect.x1_mm - a[0].max(b[0])).abs() < 1e-9, "{rect:?}");
        assert!((rect.y0_mm - a[1].min(b[1])).abs() < 1e-9, "{rect:?}");
        assert!((rect.y1_mm - a[1].max(b[1])).abs() < 1e-9, "{rect:?}");
    }
}

#[test]
fn a_thickened_feature_is_gone_on_the_next_review() {
    // A wall under a pixel, a pin under a pixel and a wall 1.5 pixels
    // thick, beside a block: all three thickened, none is left.
    let fixture = FixtureBuilder::new(16)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .model(.., Shape::rect(100.0, 20.0, 100.5, 80.0))
        .model(.., Shape::circle(130.0, 60.0, 0.3))
        .model(0..12, Shape::rect(150.0, 20.0, 151.5, 80.0))
        .build();
    let (findings, fills) = thickenings(&fixture);
    assert_eq!(findings.len(), 3, "{findings:?}");
    let picked: Vec<&CavityFill> = findings
        .iter()
        .map(|f| match f.fix {
            Some(SuggestedFix::Thicken { feature, .. }) => &fills[feature],
            other => panic!("expected a thickening, got {other:?}"),
        })
        .collect();
    let thick = crate::fill::fill_stack(picked, fixture.schedule()).expect("three features");
    let again = fixture.input_with(ReviewLeaf::new(Some("Thickened"), Role::Model, &thick));
    let regions = PlateRegions::compute(&again);
    let after = run(
        &ReviewContext::new(&again, &regions),
        &ThinFeatureOptions::default(),
    );
    assert!(after.is_empty(), "{after:?}");
    // The pin the printer dropped now lights pixels.
    assert!(!regions.layer(14).is_empty());
}

#[test]
fn a_thin_support_offers_no_thickening() {
    let fixture = FixtureBuilder::new(12)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .support(.., Shape::circle(100.0, 60.0, 0.3))
        .build();
    let (findings, fills) = thickenings(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].scope, RoleScope::Support);
    assert_eq!(findings[0].fix, None);
    assert!(fills.is_empty());
}
