/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end 3MF project round trips through the public `write_project` /
//! `read_project` entry points, using the workspace's cube and tunnel-prism
//! fixtures as the scene and profile-shaped TOML fixtures as the embedded
//! documents.

use std::io::{Cursor, Read};
use std::path::Path;

use sc_mesh_formats::filetypes::MeshFileType;
use sc_mesh_formats::load_mesh_from_reader;
use sc_mesh_formats::messages::{LoadMeshWarning, SceneLoadWarning};
use sc_mesh_formats::threemf::write_threemf;
use sc_print_primitives::{MaterialId, PrinterId};
use sc_project_formats::{
    DecompressionProgress, EmbedProfiles, EmbeddedProfile, LoadedProject, ProfileRefWarning,
    ProjectFileType, ProjectPayload, infer_project_file_type_from_extension, read_project,
    read_project_with_progress, write_project,
};
use sc_scene::Scene;
use sc_scene::node::{NodeId, NodeKind};
use sc_scene_fixtures::named_trio_under_group_root;

const TEST_PRINTER_TOML: &[u8] = include_bytes!("fixtures/test-printer.toml");
const TEST_RESIN_TOML: &[u8] = include_bytes!("fixtures/test-resin.toml");
const TEST_RESIN_EXTENSION_TOML: &[u8] = include_bytes!("fixtures/test-resin-extension.toml");

fn printer_id(raw: &str) -> PrinterId {
    PrinterId::new(raw).expect("test id is a valid slug")
}

fn material_id(raw: &str) -> MaterialId {
    MaterialId::new(raw).expect("test id is a valid slug")
}

fn full_payload() -> ProjectPayload {
    ProjectPayload {
        printer: Some(printer_id("test-printer")),
        resin: Some(material_id("test-resin")),
        first_layer_height: None,
        output_format: None,
        embedded: vec![
            EmbeddedProfile::Printer {
                id: printer_id("test-printer"),
                bytes: TEST_PRINTER_TOML.to_vec(),
            },
            EmbeddedProfile::Material {
                id: material_id("test-resin"),
                bytes: TEST_RESIN_TOML.to_vec(),
            },
            EmbeddedProfile::MaterialExtension {
                id: material_id("test-resin"),
                bytes: TEST_RESIN_EXTENSION_TOML.to_vec(),
            },
        ],
        review_decisions: Vec::new(),
        plates: 0,
        plate_names: Vec::new(),
    }
}

fn project_file_type() -> ProjectFileType {
    infer_project_file_type_from_extension(Path::new("plate.3mf"))
        .expect("the 3mf extension is a project format")
}

/// Writes the named trio and `payload` as a 3MF project.
fn write_bytes(payload: &ProjectPayload, embed: EmbedProfiles) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_project(
        &named_trio_under_group_root([0.0; 3]),
        payload,
        embed,
        None,
        None,
        project_file_type(),
        &mut Cursor::new(&mut bytes),
    )
    .expect("the project writes cleanly");
    bytes
}

fn read_back(bytes: Vec<u8>) -> LoadedProject {
    read_project(&mut Cursor::new(bytes), project_file_type()).expect("the project reads back")
}

/// Returns the children of the scene's only root, which must be a group (the
/// loader convention both readers follow).
fn root_children(scene: &Scene) -> Vec<NodeId> {
    assert_eq!(scene.roots().len(), 1);
    let root = scene.node(scene.roots()[0]).expect("root resolves");
    match &root.kind {
        NodeKind::Group { children } => children.clone(),
        other => panic!("the root should be a group, got {other:?}"),
    }
}

/// Returns the sorted mesh names of the leaves under the root group.
fn leaf_names(scene: &Scene) -> Vec<String> {
    let mut names: Vec<String> = root_children(scene)
        .into_iter()
        .map(|child| {
            let mesh = scene
                .leaf_mesh_arc(child)
                .expect("every child is a mesh leaf");
            mesh.meta()
                .as_ref()
                .and_then(|meta| meta.name.clone())
                .expect("every test mesh is named")
        })
        .collect();
    names.sort();
    names
}

mod round_trip_tests {
    use super::*;

    #[test]
    fn a_full_project_round_trips_scene_references_and_documents() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::Yes);
        let loaded = read_back(bytes);

        assert!(loaded.warnings.is_empty());
        assert_eq!(
            leaf_names(&loaded.scene),
            vec!["Cube", "YTunnel", "ZTunnel"]
        );

        let payload = loaded.payload.expect("the payload is present");
        assert_eq!(payload, full_payload());
    }

    #[test]
    fn a_references_only_project_keeps_references_and_drops_documents() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::No);
        let loaded = read_back(bytes);

        assert!(loaded.warnings.is_empty());
        let payload = loaded.payload.expect("the payload is present");
        assert_eq!(payload.printer, Some(printer_id("test-printer")));
        assert_eq!(payload.resin, Some(material_id("test-resin")));
        assert!(payload.embedded.is_empty());
    }

    #[test]
    fn an_empty_payload_round_trips_as_a_project_without_references() {
        let bytes = write_bytes(&ProjectPayload::default(), EmbedProfiles::Yes);
        let loaded = read_back(bytes);
        assert_eq!(loaded.payload, Some(ProjectPayload::default()));
    }
}

mod warning_tier_tests {
    use super::*;

    #[test]
    fn missing_profiles_warn_softly_when_the_file_embeds_them() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::Yes);
        let payload = read_back(bytes).payload.expect("the payload is present");
        assert_eq!(
            payload.profile_ref_warnings(|_| false, |_| false),
            vec![
                ProfileRefWarning::PrinterEmbeddedOnly {
                    id: printer_id("test-printer"),
                },
                ProfileRefWarning::ResinEmbeddedOnly {
                    id: material_id("test-resin"),
                },
            ],
        );
    }

    #[test]
    fn missing_profiles_warn_hard_when_the_file_is_references_only() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::No);
        let payload = read_back(bytes).payload.expect("the payload is present");
        assert_eq!(
            payload.profile_ref_warnings(|_| false, |_| false),
            vec![
                ProfileRefWarning::PrinterMissing {
                    id: printer_id("test-printer"),
                },
                ProfileRefWarning::ResinMissing {
                    id: material_id("test-resin"),
                },
            ],
        );
    }

    #[test]
    fn installed_profiles_produce_no_warnings_either_way() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::No);
        let payload = read_back(bytes).payload.expect("the payload is present");
        assert!(payload.profile_ref_warnings(|_| true, |_| true).is_empty());
    }
}

mod compatibility_tests {
    use super::*;

    #[test]
    fn a_plain_model_package_reads_as_a_project_without_a_payload() {
        let mut bytes = Vec::new();
        write_threemf(&named_trio_under_group_root([0.0; 3]), &mut bytes)
            .expect("the plain model writes");
        let loaded = read_back(bytes);

        assert!(loaded.payload.is_none());
        assert!(loaded.warnings.is_empty());
        assert_eq!(
            leaf_names(&loaded.scene),
            vec!["Cube", "YTunnel", "ZTunnel"]
        );
    }

    #[test]
    fn a_project_package_opens_in_the_plain_mesh_loader() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::Yes);
        let loaded = load_mesh_from_reader(MeshFileType::_3mf, &mut Cursor::new(bytes))
            .expect("the plain loader opens the project");

        // The tunnel prisms share their outer-corner coordinates, so the
        // loader's coincident-vertex observation always fires for this trio;
        // nothing else (in particular nothing about the project parts) may be
        // reported.
        assert!(
            loaded.warnings.iter().all(|warning| matches!(
                warning,
                LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices { .. }),
            )),
            "unexpected warnings: {:?}",
            loaded.warnings,
        );
        assert_eq!(
            leaf_names(&loaded.graph),
            vec!["Cube", "YTunnel", "ZTunnel"]
        );
    }
}

mod content_type_tests {
    use super::*;

    #[test]
    fn the_project_part_gets_an_override_when_profile_parts_share_its_extension() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::Yes);
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).expect("the package is a zip archive");
        let mut entry = archive
            .by_name("[Content_Types].xml")
            .expect("the content-types part exists");
        let mut xml = String::new();
        entry
            .read_to_string(&mut xml)
            .expect("the content-types part is UTF-8");

        // 2 content types share the toml extension, so at least the project
        // part must be declared with an Override entry.
        assert!(xml.contains("Override"));
        assert!(xml.contains("/Metadata/soulcrafted-project.toml"));
        assert!(xml.contains("application/vnd.soulcrafted.project+toml"));
        assert!(xml.contains("application/toml"));
    }
}

mod progress_tests {
    use super::*;

    #[test]
    fn container_decompression_ticks_forward_through_the_project_reader() {
        let bytes = write_bytes(&full_payload(), EmbedProfiles::Yes);
        let mut ticks: Vec<DecompressionProgress> = Vec::new();
        let loaded =
            read_project_with_progress(&mut Cursor::new(bytes.clone()), project_file_type(), |p| {
                ticks.push(p)
            })
            .expect("the project reads back");

        // Forwarding must not change what is read: same scene leaves and
        // payload as the plain entry point.
        let plain = read_back(bytes);
        assert_eq!(leaf_names(&loaded.scene), leaf_names(&plain.scene));
        assert_eq!(loaded.payload, plain.payload);

        // The container ticks keep the package layer's contract: fixed total,
        // strictly growing counts, ending at the total.
        assert!(!ticks.is_empty(), "reading a package must tick");
        let total = ticks[0].total_bytes;
        for pair in ticks.windows(2) {
            assert_eq!(pair[1].total_bytes, total, "total must stay fixed");
            assert!(
                pair[1].decompressed_bytes > pair[0].decompressed_bytes,
                "ticks must carry strictly growing counts"
            );
        }
        assert_eq!(ticks.last().expect("non-empty").decompressed_bytes, total);
    }
}

mod slice_persistence_tests {
    use super::*;

    use sc_project_formats::PlateSlices;
    use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
    use sc_sl1cer::geom2d::Point2D;

    /// Returns a CCW triangle as 1 solid layer plus an empty layer, paired with
    /// the top Z of each layer.
    fn plate_slices() -> PlateSlices {
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
            contours: std::sync::Arc::new(contours),
            slab_tops: vec![0.05, 0.10],
        }
    }

    fn write_bytes_with_slices(
        slices: Option<&PlateSlices>,
        slice_fingerprint: Option<&str>,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_project(
            &named_trio_under_group_root([0.0; 3]),
            &full_payload(),
            EmbedProfiles::Yes,
            slices,
            slice_fingerprint,
            project_file_type(),
            &mut Cursor::new(&mut bytes),
        )
        .expect("the project writes cleanly");
        bytes
    }

    #[test]
    fn a_written_slice_token_is_handed_back_on_read() {
        let plate = plate_slices();
        let loaded = read_back(write_bytes_with_slices(Some(&plate), Some("cafef00d")));
        assert_eq!(loaded.slice_fingerprint, Some("cafef00d".to_string()));
    }

    #[test]
    fn a_written_slice_part_reaches_the_package() {
        let plate = plate_slices();
        let bytes = write_bytes_with_slices(Some(&plate), Some("cafef00d"));
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).expect("the package is a zip archive");
        let carries_slices = (0..archive.len()).any(|i| {
            archive
                .by_index(i)
                .map(|entry| entry.name().contains("slices"))
                .unwrap_or(false)
        });
        assert!(
            carries_slices,
            "write_project must forward the slices into the package"
        );
    }

    #[test]
    fn writing_without_slices_leaves_no_token() {
        let loaded = read_back(write_bytes_with_slices(None, None));
        assert_eq!(loaded.slice_fingerprint, None);
    }
}

mod declared_facet_normals_tests {
    use sc_mesh_fixtures::{unit_cube_mesh, with_declared_facet_normals};

    use super::*;

    #[test]
    fn the_declared_facet_normals_survive_a_project_round_trip() {
        // Bit patterns that no geometry gives, 1 per face of the cube.
        let crafted: Vec<[u32; 3]> = (1..=12).map(|k| [k, k, k]).collect();
        let cube = with_declared_facet_normals(unit_cube_mesh(), crafted.clone());
        let mut bytes = Vec::new();
        write_project(
            &Scene::from(cube),
            &ProjectPayload::default(),
            EmbedProfiles::No,
            None,
            None,
            project_file_type(),
            &mut Cursor::new(&mut bytes),
        )
        .expect("the project writes cleanly");

        let loaded = read_back(bytes);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let leaf = loaded.scene.first_leaf_id().expect("the scene has a leaf");
        let mesh = loaded
            .scene
            .mesh_at_leaf(leaf)
            .expect("the leaf holds a mesh");
        assert_eq!(mesh.declared_facet_normals(), Some(&crafted[..]));
    }
}
