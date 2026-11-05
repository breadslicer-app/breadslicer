/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Regions, links and groups on synthetic plates.

use sc_print_primitives::Mirror;

use super::*;
use crate::test_support::{FixtureBuilder, HEIGHT_PX, Shape, WIDTH_PX};
use crate::union_find::RegionRef;

const SCREEN_PX: u64 = WIDTH_PX as u64 * HEIGHT_PX as u64;

/// The regions of a one-layer plate holding `shapes` as model.
fn one_layer(shapes: &[Shape]) -> LayerRegions {
    let mut builder = FixtureBuilder::new(1);
    for shape in shapes {
        builder = builder.model(0..1, shape.clone());
    }
    builder.build().raw_regions().layer(0).clone()
}

fn pixel(x0: u32, y0: u32) -> Shape {
    Shape::rect(
        f64::from(x0),
        f64::from(y0),
        f64::from(x0 + 1),
        f64::from(y0 + 1),
    )
}

fn region_ref(layer: usize, index: u32) -> RegionRef {
    RegionRef { layer, index }
}

#[test]
fn an_empty_layer_is_one_open_air_region() {
    let layer = one_layer(&[]);
    assert!(layer.is_empty());
    assert_eq!(layer.solid_bbox(), None);
    assert_eq!(layer.air.len(), 1);
    let air = layer.air[0];
    assert_eq!(air.area_px, SCREEN_PX);
    assert!(air.touches_border);
    assert_eq!(
        air.bbox,
        PixelBox {
            x0: 0,
            y0: 0,
            x1: WIDTH_PX,
            y1: HEIGHT_PX
        }
    );
    assert_eq!(air.centroid_px, [100.0, 60.0]);
}

#[test]
fn a_single_pixel_is_one_region() {
    let layer = one_layer(&[pixel(10, 12)]);
    assert_eq!(layer.solid.len(), 1);
    let solid = layer.solid[0];
    assert_eq!(solid.area_px, 1);
    assert_eq!(
        solid.bbox,
        PixelBox {
            x0: 10,
            y0: 12,
            x1: 11,
            y1: 13
        }
    );
    assert_eq!(solid.centroid_px, [10.5, 12.5]);
    assert_eq!(solid.inside_px, [10, 12]);
    assert!(!solid.touches_border);
    assert_eq!(layer.air.len(), 1);
    assert_eq!(layer.air[0].area_px, SCREEN_PX - 1);
}

#[test]
fn a_frame_encloses_one_air_region() {
    let layer = one_layer(&[Shape::frame(10.0, 10.0, 30.0, 30.0, 3.0)]);
    assert_eq!(layer.solid.len(), 1);
    assert_eq!(layer.solid[0].area_px, 20 * 20 - 14 * 14);
    // The centroid of a frame is its empty middle; the inside pixel is not.
    assert_eq!(layer.solid[0].centroid_px, [20.0, 20.0]);
    let [ix, iy] = layer.solid[0].inside_px;
    assert!(!(13..27).contains(&ix) || !(13..27).contains(&iy));
    assert_eq!(layer.air.len(), 2);
    assert!(layer.air[0].touches_border);
    assert_eq!(layer.air[0].area_px, SCREEN_PX - 20 * 20);
    let hole = layer.air[1];
    assert!(!hole.touches_border);
    assert_eq!(hole.area_px, 14 * 14);
    assert_eq!(
        hole.bbox,
        PixelBox {
            x0: 13,
            y0: 13,
            x1: 27,
            y1: 27
        }
    );
}

#[test]
fn nested_rings_alternate_solid_and_air() {
    let layer = one_layer(&[
        Shape::frame(10.0, 10.0, 50.0, 50.0, 4.0),
        Shape::frame(20.0, 20.0, 40.0, 40.0, 4.0),
    ]);
    let solid: Vec<u64> = layer.solid.iter().map(|r| r.area_px).collect();
    assert_eq!(solid, vec![40 * 40 - 32 * 32, 20 * 20 - 12 * 12]);
    let air: Vec<(u64, bool)> = layer
        .air
        .iter()
        .map(|r| (r.area_px, r.touches_border))
        .collect();
    assert_eq!(
        air,
        vec![
            (SCREEN_PX - 40 * 40, true),
            (32 * 32 - 20 * 20, false),
            (12 * 12, false),
        ]
    );
}

#[test]
fn corner_contact_joins_solid_and_seals_air() {
    // A diamond of 4 pixels touching only at corners around pixel (50, 50).
    let layer = one_layer(&[pixel(50, 49), pixel(49, 50), pixel(51, 50), pixel(50, 51)]);
    assert_eq!(layer.solid.len(), 1, "8-connected solid");
    assert_eq!(layer.solid[0].area_px, 4);
    assert_eq!(layer.air.len(), 2, "4-connected air");
    assert_eq!(layer.air[1].area_px, 1);
    assert_eq!(layer.air[1].inside_px, [50, 50]);
    assert!(!layer.air[1].touches_border);

    let diagonal = one_layer(&[pixel(10, 10), pixel(11, 11)]);
    assert_eq!(diagonal.solid.len(), 1);
    assert_eq!(diagonal.air.len(), 1);
}

#[test]
fn separate_shapes_are_separate_regions() {
    let layer = one_layer(&[pixel(10, 10), pixel(12, 10), pixel(10, 12)]);
    assert_eq!(layer.solid.len(), 3);
    assert_eq!(layer.air.len(), 1);
}

#[test]
fn solid_on_the_screen_edge_touches_the_border() {
    let layer = one_layer(&[
        Shape::rect(0.0, 50.0, 10.0, 60.0),
        Shape::rect(190.0, 0.0, 200.0, 5.0),
        Shape::rect(90.0, 50.0, 100.0, 60.0),
    ]);
    let touching: Vec<bool> = layer.solid.iter().map(|r| r.touches_border).collect();
    // Scan order: the top-right block (row 0) comes first.
    assert_eq!(touching, vec![true, true, false]);
}

#[test]
fn a_polygonal_circle_has_about_its_area() {
    let layer = one_layer(&[Shape::circle(100.0, 60.0, 10.0)]);
    assert_eq!(layer.solid.len(), 1);
    let area = layer.solid[0].area_px as f64;
    assert!((area - std::f64::consts::PI * 100.0).abs() < 10.0, "{area}");
    // The four pixels around the centre are equally near; scan order picks.
    let [ix, iy] = layer.solid[0].inside_px;
    assert!(
        (99..=100).contains(&ix) && (59..=60).contains(&iy),
        "{ix}, {iy}"
    );
}

#[test]
fn overlaps_link_consecutive_layers() {
    let regions = FixtureBuilder::new(2)
        .model(0..1, Shape::rect(10.0, 10.0, 30.0, 30.0))
        .model(1..2, Shape::rect(20.0, 20.0, 40.0, 40.0))
        .build()
        .regions();
    assert!(regions.solid_links(0).is_empty());
    assert!(regions.air_links(0).is_empty());
    assert_eq!(
        regions.solid_links(1),
        &[Overlap {
            lower: 0,
            upper: 0,
            pixels: 100
        }]
    );
    // Open air over open air drains whatever lies below it: no link.
    assert!(regions.air_links(1).is_empty());
    let (lower, upper) = (regions.layer(0), regions.layer(1));
    assert_eq!((lower.solid[0].below_px, lower.solid[0].above_px), (0, 100));
    assert_eq!((upper.solid[0].below_px, upper.solid[0].above_px), (100, 0));
}

#[test]
fn an_empty_layer_breaks_the_solid_links() {
    let regions = FixtureBuilder::new(3)
        .model(0..1, Shape::rect(10.0, 10.0, 30.0, 30.0))
        .model(2..3, Shape::rect(10.0, 10.0, 30.0, 30.0))
        .build()
        .regions();
    assert!(regions.layer(1).is_empty());
    assert!(regions.solid_links(1).is_empty());
    assert!(regions.solid_links(2).is_empty());
    assert_eq!(regions.layer(2).solid[0].below_px, 0);
    assert_eq!(regions.group_solid(|_, _| true).len(), 2);
}

#[test]
fn bodies_join_through_the_layers() {
    let regions = FixtureBuilder::new(3)
        .model(0..1, Shape::rect(10.0, 10.0, 20.0, 20.0))
        .model(0..1, Shape::rect(40.0, 10.0, 50.0, 20.0))
        .model(1..2, Shape::rect(10.0, 10.0, 50.0, 20.0))
        .model(2..3, Shape::rect(100.0, 100.0, 110.0, 110.0))
        .build()
        .regions();
    assert_eq!(
        regions.solid_links(1),
        &[
            Overlap {
                lower: 0,
                upper: 0,
                pixels: 100
            },
            Overlap {
                lower: 1,
                upper: 0,
                pixels: 100
            },
        ]
    );
    let bodies = regions.group_solid(|_, _| true);
    assert_eq!(bodies.len(), 2);
    assert_eq!(
        bodies.members(0),
        &[region_ref(0, 0), region_ref(0, 1), region_ref(1, 0)]
    );
    assert_eq!(bodies.members(1), &[region_ref(2, 0)]);
    assert_eq!(bodies.group_of(region_ref(0, 1)), 0);

    // A link filter splits the bridge from its right foot.
    let strict = regions.group_solid(|_, overlap| overlap.lower == 0);
    assert_eq!(strict.len(), 3);
}

#[test]
fn a_sealed_cavity_is_its_own_air_group() {
    // A closed box: floor and lid solid, walls around a cavity on layers 1-3.
    let regions = FixtureBuilder::new(5)
        .model(0..1, Shape::rect(10.0, 10.0, 30.0, 30.0))
        .model(1..4, Shape::frame(10.0, 10.0, 30.0, 30.0, 2.0))
        .model(4..5, Shape::rect(10.0, 10.0, 30.0, 30.0))
        .build()
        .regions();
    let air = regions.group_air(|_, _| true);
    let enclosed = |r: &RegionRef| !regions.layer(r.layer).air[r.index as usize].touches_border;
    let cavities: Vec<&[RegionRef]> = air
        .iter()
        .filter(|members| members.iter().any(enclosed))
        .collect();
    assert_eq!(
        cavities,
        [&[region_ref(1, 1), region_ref(2, 1), region_ref(3, 1)][..]]
    );
}

#[test]
fn leaves_of_one_role_rasterise_as_a_union() {
    let regions = FixtureBuilder::new(1)
        .shape("A", 0..1, Shape::rect(10.0, 10.0, 30.0, 20.0))
        .shape("B", 0..1, Shape::rect(20.0, 10.0, 40.0, 20.0))
        .build()
        .regions();
    let region = regions.layer(0).solid[0];
    assert_eq!(region.area_px, 300);
}

#[test]
fn mirroring_keeps_the_pixels_and_maps_back_to_the_plate() {
    let shape = Shape::rect(10.0, 10.0, 30.0, 20.0);
    let plain = FixtureBuilder::new(1).model(0..1, shape.clone()).build();
    let mirrored = FixtureBuilder::new(1)
        .mirror(Mirror { x: true, y: true })
        .model(0..1, shape)
        .build();
    let a = plain.regions().layer(0).solid[0];
    let b = mirrored.regions().layer(0).solid[0];
    assert_eq!(a, b);
    let a_mm = plain.input().focus(0, a.centroid_px);
    let b_mm = mirrored.input().focus(0, b.centroid_px);
    assert!((a_mm.x_mm + 8.0).abs() < 1e-9, "{a_mm:?}");
    assert!((b_mm.x_mm - 8.0).abs() < 1e-9, "{b_mm:?}");
    assert!((a_mm.y_mm + b_mm.y_mm).abs() < 1e-9);
    assert_eq!(a_mm.z_mm, 0.025);
}

#[test]
fn links_cross_every_batch_boundary() {
    // More layers than one batch holds, whatever the thread count.
    let layers = rayon::current_num_threads() * 4 * 3 + 5;
    let regions = FixtureBuilder::new(layers)
        .model(.., Shape::rect(10.0, 10.0, 20.0, 30.0))
        .max_z_mm(1000.0)
        .build()
        .regions();
    for layer in 1..layers {
        assert_eq!(
            regions.solid_links(layer),
            &[Overlap {
                lower: 0,
                upper: 0,
                pixels: 200
            }],
            "layer {layer}"
        );
        assert_eq!(regions.layer(layer).solid[0].below_px, 200);
        assert_eq!(regions.layer(layer - 1).solid[0].above_px, 200);
    }
    assert_eq!(regions.group_solid(|_, _| true).len(), 1);
}
