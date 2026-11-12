/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_print_glue::slice_input_fingerprint;
use sc_print_primitives::{MaterialId, PrinterId, ScaleCompensation};
use sc_print_profiles::resolve_scale_compensation;
use sc_project_formats::{
    EmbedProfiles, PlateSlices, ProjectFileType, ProjectPayload,
    infer_project_file_type_from_extension, write_project,
};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;

use crate::slice_projection::{load_selected_profiles, resolve_current_schedule};

/// Returns the scale compensation that the installed `printer` and `resin`
/// resolve to, which a slice of them is cut under and fingerprinted with.
pub(crate) fn resolved_scale(printer: &PrinterId, resin: &MaterialId) -> ScaleCompensation {
    let (printer, resin) =
        load_selected_profiles(printer, resin).expect("the installed profiles load");
    resolve_scale_compensation(&printer, &resin)
}

/// Returns the corners of a 2 mm cube on the floor, meshed by its 2 floor
/// triangles only.
pub(crate) fn cube_mesh() -> IndexedMesh {
    let v = |x, y, z| Vertex::new([x, y, z]);
    let vertices = vec![
        v(-1.0, -1.0, 0.0),
        v(1.0, -1.0, 0.0),
        v(1.0, 1.0, 0.0),
        v(-1.0, 1.0, 0.0),
        v(-1.0, -1.0, 2.0),
        v(1.0, -1.0, 2.0),
        v(1.0, 1.0, 2.0),
        v(-1.0, 1.0, 2.0),
    ];
    let faces = vec![
        IndexedTriangle::from_raw_indices([0, 1, 2]),
        IndexedTriangle::from_raw_indices([0, 2, 3]),
    ];
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

pub(crate) fn three_mf() -> ProjectFileType {
    infer_project_file_type_from_extension(Path::new("plate.3mf"))
        .expect("the 3mf extension is a project format")
}

/// Returns the bytes of a 3MF project whose persisted slices are in sync with
/// its scene, and the slice token that the file stores.
///
/// The saved scene has 1 mesh leaf as its root, as the plain-mesh loader builds
/// it, with a placement written onto the leaf: a non-identity affine transform
/// (a fit scale plus recentre). Its slice token comes from the schedule that
/// the bundled default profiles resolve, and the file stores hand-built
/// contours: an empty first layer and a square on every other layer. Reopening
/// from 3MF wraps the leaf under a fresh identity group, so the saved and the
/// reopened scene place the same geometry under different nesting.
pub(crate) fn project_with_in_sync_slices() -> (Vec<u8>, String) {
    let printer = PrinterId::new("elegoo-mars-5").expect("valid id");
    let resin = MaterialId::new("siraya-tech-craft-high-clear").expect("valid id");

    let placement = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        [
            [0.6173767447471619, 0.0, 0.0],
            [0.0, 0.6173767447471619, 0.0],
            [0.0, 0.0, 0.6173767447471619],
        ],
        [
            -67.91707062404612,
            -67.90612409423966,
            0.0021667425876330526,
        ],
    ));
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(None, placement, LeafPayload::Mesh(Arc::new(cube_mesh())))
        .expect("add leaf");
    scene.add_root(leaf).expect("leaf becomes the scene root");

    let schedule = resolve_current_schedule(&printer, &resin, None, &scene)
        .expect("the bundled default profiles resolve a schedule");
    let token = slice_input_fingerprint(&scene, &schedule, resolved_scale(&printer, &resin))
        .expect("the scene has sliceable geometry")
        .to_token();

    let slab_tops: Vec<f64> = schedule.iter_slab_tops().collect();
    let square = Polygon::new(
        ClosedRing::new(vec![
            Point2D::new(-1.0, -1.0),
            Point2D::new(1.0, -1.0),
            Point2D::new(1.0, 1.0),
            Point2D::new(-1.0, 1.0),
        ]),
        vec![],
    )
    .expect("a CCW square is a valid polygon");
    let layers: Vec<ContourLayer> = slab_tops
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let polys = if i == 0 { vec![] } else { vec![square.clone()] };
            ContourLayer::new(0.05, polys)
        })
        .collect();
    let plate = PlateSlices {
        contours: Arc::new(ContourStack::new_unchecked(layers)),
        slab_tops,
    };

    let payload = ProjectPayload {
        printer: Some(printer.clone()),
        resin: Some(resin.clone()),
        first_layer_height: None,
        output_format: None,
        embedded: Vec::new(),
        review_decisions: Vec::new(),
        plates: 0,
        plate_names: Vec::new(),
        hand_placed: Vec::new(),
    };

    let mut bytes = Vec::new();
    write_project(
        &scene,
        &payload,
        EmbedProfiles::No,
        Some(&plate),
        Some(&token),
        three_mf(),
        &mut Cursor::new(&mut bytes),
    )
    .expect("the project writes cleanly");
    (bytes, token)
}
