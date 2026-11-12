/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Fixtures shared by the tests of the 3MF project writer.

use std::sync::Arc;

use sc_3mf::package::PartName;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_fixtures::unit_cube_mesh;
use sc_print_primitives::{MaterialId, PrinterId};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;

use crate::payload::{EmbeddedProfile, ProjectPayload};
use crate::slices::PlateSlices;

pub(super) fn printer_id(raw: &str) -> PrinterId {
    PrinterId::new(raw).expect("test id is a valid slug")
}

pub(super) fn material_id(raw: &str) -> MaterialId {
    MaterialId::new(raw).expect("test id is a valid slug")
}

pub(super) fn cube_scene() -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("adding an inline mesh leaf succeeds");
    scene
        .add_root(leaf)
        .expect("a fresh leaf can become a root");
    scene
}

/// Returns a CCW triangle as 1 solid layer plus 1 empty layer, paired with the
/// top Z of each layer.
pub(super) fn plate_slices() -> PlateSlices {
    let outer = ClosedRing::new(
        [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)]
            .into_iter()
            .map(|(x, y)| Point2D::new(x, y))
            .collect(),
    );
    let triangle = Polygon::new(outer, vec![]).expect("the outer ring is CCW");
    let solid = ContourLayer::new(0.025, vec![triangle]);
    let void = ContourLayer::new(0.075, vec![]);
    let contours = ContourStack::new(vec![solid, void]).expect("z is strictly increasing");
    PlateSlices {
        contours: Arc::new(contours),
        slab_tops: vec![0.05, 0.10],
    }
}

pub(super) fn full_payload() -> ProjectPayload {
    ProjectPayload {
        printer: Some(printer_id("test-printer")),
        resin: Some(material_id("test-resin")),
        first_layer_height: None,
        output_format: None,
        embedded: vec![
            EmbeddedProfile::Printer {
                id: printer_id("test-printer"),
                bytes: b"kind = \"printer\"".to_vec(),
            },
            EmbeddedProfile::Material {
                id: material_id("test-resin"),
                bytes: b"kind = \"material\"".to_vec(),
            },
            EmbeddedProfile::MaterialExtension {
                id: material_id("test-resin"),
                bytes: b"kind = \"extension\"".to_vec(),
            },
        ],
        review_decisions: Vec::new(),
        plates: 0,
        plate_names: Vec::new(),
        hand_placed: Vec::new(),
    }
}

pub(super) fn part(name: &str) -> PartName {
    PartName::new(name).expect("test part name is valid")
}
