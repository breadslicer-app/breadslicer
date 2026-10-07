/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel};
use sc_print_primitives::BuildVolume;
use sc_scene::Scene;
use sc_scene_fixtures::{meshes_as_roots, trio_as_roots};

use super::placement::place_on_plate;

/// Returns a 100 x 60 x 80 mm volume; the centre of its floor is the origin.
fn volume() -> BuildVolume {
    BuildVolume::new(100.0, 60.0, 80.0).expect("positive finite lengths")
}

fn translation(offset: [f64; 3]) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_translation(offset))
}

/// Returns the trio of sc-scene-fixtures with its roots moved by `offsets`, in
/// order.
fn moved_trio(offsets: [[f64; 3]; 3]) -> Scene {
    let mut scene = trio_as_roots();
    for (root, offset) in scene.roots().to_vec().into_iter().zip(offsets) {
        scene
            .set_local_transform(root, translation(offset))
            .expect("a root of the scene");
    }
    scene
}

fn root_transforms(scene: &Scene) -> Vec<Transform3D> {
    scene
        .roots()
        .iter()
        .map(|&root| {
            scene
                .node(root)
                .expect("a root of the scene")
                .local_transform
        })
        .collect()
}

fn world_box(scene: &Scene) -> ([f32; 3], [f32; 3]) {
    let aabb = scene
        .aabb()
        .expect("a scene of these tests has no dangling id")
        .expect("the scene has geometry");
    (aabb.min().coords(), aabb.max().coords())
}

#[test]
fn a_scene_off_the_origin_ends_centred_over_the_plate_on_its_floor() {
    let mut scene = moved_trio([
        [30.0, -20.0, 15.0],
        [34.0, -20.0, 15.0],
        [26.0, -20.0, 15.0],
    ]);
    assert_eq!(
        world_box(&scene),
        ([25.0, -21.0, 14.5], [35.0, -19.0, 17.0])
    );

    place_on_plate(&mut scene, &volume()).expect("the scene places");

    assert_eq!(world_box(&scene), ([-5.0, -1.0, 0.0], [5.0, 1.0, 2.5]));
    assert_eq!(
        root_transforms(&scene),
        [
            translation([0.0, 0.0, 0.5]),
            translation([4.0, 0.0, 0.5]),
            translation([-4.0, 0.0, 0.5]),
        ]
    );
}

#[test]
fn a_scene_already_on_the_plate_keeps_its_exact_transforms() {
    let mut scene = meshes_as_roots([cuboid_with_z_tunnel(), cuboid_with_y_tunnel()]);
    assert_eq!(world_box(&scene), ([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]));

    place_on_plate(&mut scene, &volume()).expect("the scene places");

    assert_eq!(root_transforms(&scene), [Transform3D::Identity; 2]);
}

#[test]
fn a_scene_without_geometry_is_left_as_it_is() {
    let mut empty = Scene::new();
    place_on_plate(&mut empty, &volume()).expect("an empty scene places");
    assert!(empty.roots().is_empty());

    let mut empty_group = Scene::new();
    let group = empty_group.add_group(None, Transform3D::Identity);
    empty_group
        .add_root(group)
        .expect("a fresh group becomes a root");
    place_on_plate(&mut empty_group, &volume()).expect("a scene without geometry places");
    assert_eq!(root_transforms(&empty_group), [Transform3D::Identity]);
}

#[test]
fn a_lifted_scene_is_placed_too() {
    // A scene above the floor goes down to it, whatever placement its file
    // saved.
    let mut scene = moved_trio([[0.0, 0.0, 10.0]; 3]);
    assert_eq!(world_box(&scene), ([-1.0, -1.0, 9.5], [1.0, 1.0, 12.0]));

    place_on_plate(&mut scene, &volume()).expect("the scene places");

    assert_eq!(world_box(&scene), ([-1.0, -1.0, 0.0], [1.0, 1.0, 2.5]));
}
