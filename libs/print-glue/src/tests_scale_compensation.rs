/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_print_primitives::{AaLevel, PositiveFiniteMm};
use sc_print_profiles::{EffectiveMaterial, MaterialProfile, PrinterProfile};
use sc_scene::Scene;
use sc_scene::node::NodeId;
use sc_scene_fixtures::{leaves_as_roots, leaves_under_group_root};
use sc_sl1cer::CancelToken;
use sc_sl1cer::contour::ContourStack;

use super::scale_compensation::SupportOwners;
use super::slice_plate::{
    SliceRun, SlicedPlate, SlicingParams, schedule_for_selection, slice_plate,
};

const PRINTER_TOML: &str = r#"
[identity]
id = "elegoo-saturn-4-ultra"
manufacturer = "ELEGOO"
family = "Saturn"
name = "Saturn 4 Ultra"
profile_version = "1.0.0"

[hardware]
build_volume = { x_mm = 153.36, y_mm = 77.76, z_mm = 165.0 }
pixel_resolution = { x = 8520, y = 4320 }
pixel_size_microns = { x = 18.0, y = 18.0 }
mirror = { x = true, y = false }

[capabilities]
output_formats = [{ format = "ctb", version = "4" }]
supports_grayscale = true
two_stage_lift = true
has_camera = false

[constraints]
"#;

/// A material whose defaults hold `scale`, an inline TOML table, or no
/// scale compensation at all when `scale` is empty.
fn material(scale: &str) -> EffectiveMaterial {
    let scale = if scale.is_empty() {
        String::new()
    } else {
        format!("scale_compensation_pct = {scale}\n")
    };
    let toml = format!(
        r#"
[identity]
id = "standard-405"
brand = "Default"
family = "standard"
name = "Standard 405nm"
profile_version = "1.0.0"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5
{scale}
[per_printer]
"#
    );
    let profile: MaterialProfile = toml_edit::de::from_str(&toml).expect("material parses");
    EffectiveMaterial::from(profile)
}

fn printer() -> PrinterProfile {
    toml_edit::de::from_str(PRINTER_TOML).expect("printer fixture parses")
}

fn params() -> SlicingParams {
    SlicingParams {
        layer_height_request: Some(PositiveFiniteMm::new(0.5).expect("positive")),
        first_layer_height: None,
        aa_level: AaLevel::Off,
    }
}

/// A `size` mm cube standing on the plate, centred on `(x, y)`.
fn cube_at(x: f64, y: f64, size: f64) -> (IndexedMesh, Transform3D) {
    let place = AffineTransform3D::from_translation([x, y, size / 2.0])
        .compose(&AffineTransform3D::from_uniform_scale(size));
    (unit_cube_mesh(), Transform3D::Affine(place))
}

fn slice(scene: &Scene, material: &EffectiveMaterial, supports: &SupportOwners) -> SlicedPlate {
    match slice_plate(
        scene,
        &printer(),
        material,
        &params(),
        supports,
        &CancelToken::new(),
        |_| {},
    )
    .expect("the scene slices")
    {
        SliceRun::Completed(plate) => plate,
        SliceRun::Cancelled => panic!("an uncancelled run cannot report Cancelled"),
    }
}

/// The bounds `[min_x, min_y, max_x, max_y]` of a stack's first layer.
fn bounds(stack: &ContourStack) -> [f64; 4] {
    let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for polygon in stack.layers()[0].polygons() {
        for point in polygon.outer().vertices() {
            bounds[0] = bounds[0].min(point.x);
            bounds[1] = bounds[1].min(point.y);
            bounds[2] = bounds[2].max(point.x);
            bounds[3] = bounds[3].max(point.y);
        }
    }
    bounds
}

fn assert_near(actual: [f64; 4], expected: [f64; 4]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-4, "{actual:?} is not {expected:?}");
    }
}

fn leaf_ids(scene: &Scene) -> Vec<NodeId> {
    sc_sl1cer::sliceable_leaves(scene).expect("the scene has leaves")
}

#[test]
fn no_compensation_slices_the_scene_as_it_is() {
    let scene = leaves_as_roots([cube_at(-20.0, 0.0, 10.0), cube_at(20.0, 5.0, 10.0)]);
    let unset = slice(&scene, &material(""), &SupportOwners::new());
    let zero = slice(
        &scene,
        &material("{ x = 0.0, y = 0.0, z = 0.0 }"),
        &SupportOwners::new(),
    );
    assert_eq!(unset.leaves(), zero.leaves());
    assert_eq!(unset.schedule(), zero.schedule());
    assert_near(bounds(zero.leaves()[0].stack()), [-25.0, -5.0, -15.0, 5.0]);
}

#[test]
fn each_model_grows_about_its_own_centre() {
    let scene = leaves_as_roots([cube_at(-20.0, 0.0, 10.0), cube_at(20.0, 5.0, 10.0)]);
    let plate = slice(
        &scene,
        &material("{ x = 1.0, y = 2.0, z = 0.0 }"),
        &SupportOwners::new(),
    );
    assert_near(
        bounds(plate.leaves()[0].stack()),
        [-25.05, -5.1, -14.95, 5.1],
    );
    assert_near(
        bounds(plate.leaves()[1].stack()),
        [14.95, -0.1, 25.05, 10.1],
    );
}

#[test]
fn supports_scale_about_their_model_centre() {
    // A model at x = 0, and its "support" set off to the side at x = 10.
    let scene = leaves_as_roots([cube_at(0.0, 0.0, 10.0), cube_at(10.0, 0.0, 2.0)]);
    let leaves = leaf_ids(&scene);
    let supports: SupportOwners = [(leaves[1], Some(leaves[0]))].into_iter().collect();
    let plate = slice(
        &scene,
        &material("{ x = 1.0, y = 1.0, z = 0.0 }"),
        &supports,
    );
    // About the model's centre at x = 0: 9 → 9.09 and 11 → 11.11.
    assert_near(
        bounds(plate.leaves()[1].stack()),
        [9.09, -1.01, 11.11, 1.01],
    );
}

#[test]
fn a_support_with_no_known_model_goes_with_the_one_it_stands_under() {
    // The support sits under the right model's footprint.
    let scene = leaves_as_roots([
        cube_at(-20.0, 0.0, 10.0),
        cube_at(20.0, 0.0, 10.0),
        cube_at(22.0, 0.0, 2.0),
    ]);
    let leaves = leaf_ids(&scene);
    let supports: SupportOwners = [(leaves[2], None)].into_iter().collect();
    let plate = slice(
        &scene,
        &material("{ x = 1.0, y = 1.0, z = 0.0 }"),
        &supports,
    );
    // About x = 20: 21 → 21.01 and 23 → 23.03.
    assert_near(
        bounds(plate.leaves()[2].stack()),
        [21.01, -1.01, 23.03, 1.01],
    );
}

#[test]
fn a_leaf_in_a_moved_group_scales_about_its_world_centre() {
    let mut scene = leaves_under_group_root([cube_at(5.0, 0.0, 10.0)]);
    let group = scene.roots()[0];
    scene
        .set_local_transform(
            group,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
        )
        .expect("the group exists");
    let plate = slice(
        &scene,
        &material("{ x = 10.0, y = 0.0, z = 0.0 }"),
        &SupportOwners::new(),
    );
    assert_near(bounds(plate.leaves()[0].stack()), [9.5, -5.0, 20.5, 5.0]);
}

#[test]
fn z_scales_about_the_plate_and_the_schedule_follows() {
    // A 10 mm cube cut in 0.5 mm layers: 20 layers, 22 once 10 % taller.
    let scene = leaves_as_roots([cube_at(30.0, 0.0, 10.0)]);
    let material = material("{ x = 0.0, y = 0.0, z = 10.0 }");
    let plate = slice(&scene, &material, &SupportOwners::new());
    assert_eq!(plate.schedule().total_layers(), 22);
    let top = plate.leaves()[0].stack().layers().last().expect("layers");
    assert_eq!(top.polygons().len(), 1, "the top layer still cuts the cube");
    let schedule = schedule_for_selection(&scene, &printer(), &material, &params())
        .expect("the schedule resolves");
    assert_eq!(&schedule, plate.schedule());
    assert!((plate.compensated_height_mm(2.0) - 2.2).abs() < 1e-12);
}
