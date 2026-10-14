/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::Cursor;

use sc_3mf::error::PackageWriteError;
use sc_3mf::model::parse_model;
use sc_3mf::model::resource_ids::ResourceId;
use sc_3mf::names::{MODEL_PART_CONTENT_TYPE, MUST_PRESERVE_REL_TYPE, START_PART_REL_TYPE};
use sc_3mf::package::Package;

use crate::error::WriteThreeMfProjectError;
use crate::payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload};
use crate::slices::PlateSlices;
use crate::threemf::payload_part::{PayloadParseOutcome, parse_payload};
use crate::threemf::slice_mapping::slice_stack_from_contours;
use crate::threemf::test_fixtures::{
    cube_scene, full_payload, material_id, part, plate_slices, printer_id,
};
use crate::threemf::writer::{write_threemf_project, write_threemf_project_with_progress};
use crate::threemf::{
    PROFILE_PART_CONTENT_TYPE, PROJECT_PART_CONTENT_TYPE, PROJECT_PART_NAME, PROJECT_REL_TYPE,
    SLICE_PART_NAME, SLICE_REL_TYPE,
};
use crate::write::WriteProgress;

fn write_and_reopen(payload: &ProjectPayload, embed: EmbedProfiles) -> Package {
    let mut bytes = Vec::new();
    write_threemf_project(&cube_scene(), payload, embed, None, None, &mut bytes)
        .expect("the project writes cleanly");
    Package::read_from(Cursor::new(bytes)).expect("the written package reopens")
}

mod package_shape_tests {
    use super::*;

    #[test]
    fn the_package_carries_model_project_and_profile_parts() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::Yes);
        assert_eq!(
            package.part_content_type(&part("/3D/3dmodel.model")),
            Some(MODEL_PART_CONTENT_TYPE),
        );
        assert_eq!(
            package.part_content_type(&part(PROJECT_PART_NAME)),
            Some(PROJECT_PART_CONTENT_TYPE),
        );
        for name in [
            "/Metadata/profiles/printers/test-printer.toml",
            "/Metadata/profiles/materials/test-resin.toml",
            "/Metadata/profiles/materials/extensions/test-resin.toml",
        ] {
            assert_eq!(
                package.part_content_type(&part(name)),
                Some(PROFILE_PART_CONTENT_TYPE),
                "for {name}",
            );
        }
    }

    #[test]
    fn embedded_parts_carry_the_document_bytes_unchanged() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::Yes);
        assert_eq!(
            package.part_data(&part("/Metadata/profiles/printers/test-printer.toml")),
            Some(b"kind = \"printer\"".as_slice()),
        );
        assert_eq!(
            package.part_data(&part("/Metadata/profiles/materials/test-resin.toml")),
            Some(b"kind = \"material\"".as_slice()),
        );
        assert_eq!(
            package.part_data(&part(
                "/Metadata/profiles/materials/extensions/test-resin.toml"
            )),
            Some(b"kind = \"extension\"".as_slice()),
        );
    }

    #[test]
    fn the_project_part_parses_back_to_the_references() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::Yes);
        let bytes = package
            .part_data(&part(PROJECT_PART_NAME))
            .expect("the project part exists");
        let text = str::from_utf8(bytes).expect("the project part is UTF-8");
        assert_eq!(
            parse_payload(text).expect("the project part parses"),
            PayloadParseOutcome::V1 {
                printer: Some(printer_id("test-printer")),
                resin: Some(material_id("test-resin")),
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn an_empty_payload_still_writes_a_versioned_project_part() {
        let package = write_and_reopen(&ProjectPayload::default(), EmbedProfiles::Yes);
        let bytes = package
            .part_data(&part(PROJECT_PART_NAME))
            .expect("the project part exists");
        let text = str::from_utf8(bytes).expect("the project part is UTF-8");
        assert_eq!(
            parse_payload(text).expect("the project part parses"),
            PayloadParseOutcome::V1 {
                printer: None,
                resin: None,
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }

    #[test]
    fn a_supplied_slice_fingerprint_is_written_into_the_project_part() {
        let mut bytes = Vec::new();
        write_threemf_project(
            &cube_scene(),
            &full_payload(),
            EmbedProfiles::Yes,
            None,
            Some("cafef00d"),
            &mut bytes,
        )
        .expect("the project writes cleanly");
        let package = Package::read_from(Cursor::new(bytes)).expect("the written package reopens");
        let text = str::from_utf8(
            package
                .part_data(&part(PROJECT_PART_NAME))
                .expect("the project part exists"),
        )
        .expect("the project part is UTF-8");
        assert!(matches!(
            parse_payload(text).expect("the project part parses"),
            PayloadParseOutcome::V1 {
                slice_fingerprint: Some(token),
                ..
            } if token == "cafef00d",
        ));
    }
}

mod relationship_tests {
    use super::*;

    #[test]
    fn the_root_links_the_model_the_project_part_and_its_preservation() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::Yes);
        let rels = package.root_relationships();
        assert!(
            rels.iter()
                .any(|r| r.rel_type == START_PART_REL_TYPE && r.target == "/3D/3dmodel.model"),
        );
        assert!(
            rels.iter()
                .any(|r| r.rel_type == PROJECT_REL_TYPE && r.target == PROJECT_PART_NAME),
        );
        assert!(
            rels.iter()
                .any(|r| r.rel_type == MUST_PRESERVE_REL_TYPE && r.target == PROJECT_PART_NAME),
        );
    }

    #[test]
    fn every_embedded_part_is_marked_must_preserve() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::Yes);
        let rels = package.root_relationships();
        for target in [
            "/Metadata/profiles/printers/test-printer.toml",
            "/Metadata/profiles/materials/test-resin.toml",
            "/Metadata/profiles/materials/extensions/test-resin.toml",
        ] {
            assert!(
                rels.iter()
                    .any(|r| r.rel_type == MUST_PRESERVE_REL_TYPE && r.target == target),
                "for {target}",
            );
        }
    }

    #[test]
    fn relationship_ids_are_unique() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::Yes);
        let rels = package.root_relationships();
        let mut ids: Vec<&str> = rels.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }
}

mod embed_flag_tests {
    use super::*;

    #[test]
    fn references_only_files_carry_no_profile_parts() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::No);
        assert!(
            !package
                .part_names()
                .any(|name| name.as_str().starts_with("/Metadata/profiles/")),
        );
        assert!(
            !package
                .root_relationships()
                .iter()
                .any(|r| r.target.starts_with("/Metadata/profiles/")),
        );
    }

    #[test]
    fn references_only_files_keep_the_references() {
        let package = write_and_reopen(&full_payload(), EmbedProfiles::No);
        let bytes = package
            .part_data(&part(PROJECT_PART_NAME))
            .expect("the project part exists");
        let text = str::from_utf8(bytes).expect("the project part is UTF-8");
        assert_eq!(
            parse_payload(text).expect("the project part parses"),
            PayloadParseOutcome::V1 {
                printer: Some(printer_id("test-printer")),
                resin: Some(material_id("test-resin")),
                first_layer_height: None,
                output_format: None,
                slice_fingerprint: None,
            },
        );
    }
}

mod failure_tests {
    use super::*;

    #[test]
    fn two_embedded_documents_of_one_kind_and_id_collide() {
        let mut payload = full_payload();
        payload.embedded.push(EmbeddedProfile::Printer {
            id: printer_id("test-printer"),
            bytes: b"duplicate = true".to_vec(),
        });
        let mut bytes = Vec::new();
        let err = write_threemf_project(
            &cube_scene(),
            &payload,
            EmbedProfiles::Yes,
            None,
            None,
            &mut bytes,
        )
        .expect_err("duplicate part names are refused");
        assert!(matches!(
            err,
            WriteThreeMfProjectError::Package(PackageWriteError::DuplicatePartName { .. }),
        ));
    }
}

mod slice_part_tests {
    use super::*;

    /// The id the writer assigns to the single stack in the slice part.
    fn slice_stack_id() -> ResourceId {
        ResourceId::MIN
    }

    fn write_with_slices(plate: Option<&PlateSlices>) -> Package {
        let mut bytes = Vec::new();
        write_threemf_project(
            &cube_scene(),
            &full_payload(),
            EmbedProfiles::Yes,
            plate,
            None,
            &mut bytes,
        )
        .expect("the project writes cleanly");
        Package::read_from(Cursor::new(bytes)).expect("the written package reopens")
    }

    #[test]
    fn the_slice_part_carries_the_mapped_slice_stack() {
        let plate = plate_slices();
        let package = write_with_slices(Some(&plate));

        assert_eq!(
            package.part_content_type(&part(SLICE_PART_NAME)),
            Some(MODEL_PART_CONTENT_TYPE),
        );
        let bytes = package
            .part_data(&part(SLICE_PART_NAME))
            .expect("the slice part exists");
        let parsed = parse_model(bytes).expect("the slice part parses back");
        let expected =
            slice_stack_from_contours(&plate.contours, &plate.slab_tops, slice_stack_id())
                .expect("the fixture maps cleanly");
        assert_eq!(parsed.slice_stacks, vec![expected]);
    }

    #[test]
    fn the_slice_part_reads_back_to_a_contour_stack() {
        let plate = plate_slices();
        let mut bytes = Vec::new();
        write_threemf_project(
            &cube_scene(),
            &full_payload(),
            EmbedProfiles::Yes,
            Some(&plate),
            None,
            &mut bytes,
        )
        .expect("the project writes cleanly");

        let loaded = crate::threemf::reader::read_threemf_project(&mut Cursor::new(bytes))
            .expect("the project reads back");
        let slices = loaded.slices.expect("the slice geometry is read back");
        assert_eq!(slices.layers().len(), 2);
        // The solid triangle layer returns as 1 polygon; the void layer returns
        // with none.
        assert_eq!(slices.layers()[0].polygons().len(), 1);
        assert!(slices.layers()[1].polygons().is_empty());
    }

    #[test]
    fn a_project_without_slices_reads_back_no_slice_geometry() {
        let mut bytes = Vec::new();
        write_threemf_project(
            &cube_scene(),
            &full_payload(),
            EmbedProfiles::Yes,
            None,
            None,
            &mut bytes,
        )
        .expect("the project writes cleanly");

        let loaded = crate::threemf::reader::read_threemf_project(&mut Cursor::new(bytes))
            .expect("the project reads back");
        assert!(loaded.slices.is_none());
    }

    #[test]
    fn the_root_links_and_preserves_the_slice_part() {
        let package = write_with_slices(Some(&plate_slices()));
        let rels = package.root_relationships();
        assert!(
            rels.iter()
                .any(|r| r.rel_type == SLICE_REL_TYPE && r.target == SLICE_PART_NAME),
        );
        assert!(
            rels.iter()
                .any(|r| r.rel_type == MUST_PRESERVE_REL_TYPE && r.target == SLICE_PART_NAME),
        );
    }

    #[test]
    fn omitting_slices_leaves_the_model_part_untouched() {
        let with = write_with_slices(Some(&plate_slices()));
        let without = write_with_slices(None);

        // The scene model part is byte-identical whether or not slices are
        // written alongside it.
        assert_eq!(
            with.part_data(&part("/3D/3dmodel.model")),
            without.part_data(&part("/3D/3dmodel.model")),
        );
        // Without slices there is no slice part and no slice link.
        assert!(without.part_data(&part(SLICE_PART_NAME)).is_none());
        assert!(
            !without
                .root_relationships()
                .iter()
                .any(|r| r.rel_type == SLICE_REL_TYPE),
        );
    }
}

mod progress_tests {
    use super::*;

    fn write_collecting_progress(plate: Option<&PlateSlices>) -> (Vec<u8>, Vec<WriteProgress>) {
        let mut bytes = Vec::new();
        let mut events = Vec::new();
        write_threemf_project_with_progress(
            &cube_scene(),
            &full_payload(),
            EmbedProfiles::Yes,
            plate,
            None,
            &mut bytes,
            |p| events.push(p),
        )
        .expect("the project writes cleanly");
        (bytes, events)
    }

    /// Returns the `(done, total)` pair of every serialize report, in order.
    fn serialize_reports(events: &[WriteProgress]) -> Vec<(u64, u64)> {
        events
            .iter()
            .filter_map(|e| match e {
                WriteProgress::Serialize { done, total } => Some((*done, *total)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_progress_variant_writes_the_same_bytes_as_the_plain_writer() {
        let (with_progress, _) = write_collecting_progress(None);
        let mut plain = Vec::new();
        write_threemf_project(
            &cube_scene(),
            &full_payload(),
            EmbedProfiles::Yes,
            None,
            None,
            &mut plain,
        )
        .expect("the project writes cleanly");
        assert_eq!(
            with_progress, plain,
            "reporting progress must not alter output"
        );
    }

    #[test]
    fn both_phases_fire_and_serialize_precedes_compress() {
        let (_, events) = write_collecting_progress(None);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WriteProgress::Serialize { .. })),
            "the serialize phase must report at least once",
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WriteProgress::Compress { .. })),
            "the compress phase must report at least once",
        );
        let first_compress = events
            .iter()
            .position(|e| matches!(e, WriteProgress::Compress { .. }))
            .expect("a compress event exists");
        assert!(
            !events[first_compress..]
                .iter()
                .any(|e| matches!(e, WriteProgress::Serialize { .. })),
            "no serialize event may follow the first compress event",
        );
    }

    #[test]
    fn compress_counts_are_monotone_and_end_at_a_nonzero_total() {
        let (_, events) = write_collecting_progress(Some(&plate_slices()));
        let compress: Vec<(u64, u64)> = events
            .iter()
            .filter_map(|e| match e {
                WriteProgress::Compress { done, total } => Some((*done, *total)),
                _ => None,
            })
            .collect();
        assert!(!compress.is_empty(), "compressing must report");
        let total = compress[0].1;
        assert!(total > 0, "the compress total covers the part bytes");
        for pair in compress.windows(2) {
            assert_eq!(pair[1].1, total, "the compress total stays fixed");
            assert!(pair[1].0 >= pair[0].0, "compress counts never regress");
        }
        assert_eq!(
            compress.last().unwrap().0,
            total,
            "compression ends at total"
        );
    }

    #[test]
    fn serialize_reports_once_per_document() {
        // No slices: only the mesh model is serialized, so exactly 1 serialize
        // report fires at that document's boundary.
        let (_, mesh_only) = write_collecting_progress(None);
        let mesh_serialize = mesh_only
            .iter()
            .filter(|e| matches!(e, WriteProgress::Serialize { .. }))
            .count();
        assert_eq!(mesh_serialize, 1, "one document, one serialize report");

        // With slices, the mesh and the slice model are each serialized, so the
        // boundary report fires 2 times.
        let (_, with_slices) = write_collecting_progress(Some(&plate_slices()));
        let sliced_serialize = with_slices
            .iter()
            .filter(|e| matches!(e, WriteProgress::Serialize { .. }))
            .count();
        assert_eq!(sliced_serialize, 2, "two documents, two serialize reports");
    }

    #[test]
    fn serialize_counts_carry_a_nonzero_estimate_and_never_regress() {
        let (_, events) = write_collecting_progress(Some(&plate_slices()));
        let serialize = serialize_reports(&events);
        assert!(!serialize.is_empty());
        for (done, total) in &serialize {
            assert!(*total > 0, "the serialize estimate is nonzero");
            assert!(*done > 0, "serialized bytes accumulate");
        }
        for pair in serialize.windows(2) {
            assert_eq!(pair[1].1, serialize[0].1, "the estimate stays fixed");
            assert!(pair[1].0 >= pair[0].0, "serialize counts never regress");
        }
    }

    #[test]
    fn serialize_counts_end_at_the_model_part_lengths() {
        let (bytes, events) = write_collecting_progress(Some(&plate_slices()));
        let package = Package::read_from(Cursor::new(bytes)).expect("the written package reopens");
        let length = |name: &str| {
            package
                .part_data(&part(name))
                .expect("the part is present")
                .len() as u64
        };
        let model = length("/3D/3dmodel.model");
        let slice = length(SLICE_PART_NAME);
        let counts: Vec<u64> = serialize_reports(&events)
            .into_iter()
            .map(|(done, _)| done)
            .collect();
        assert_eq!(counts, vec![model, model + slice]);
    }

    #[test]
    fn slices_raise_the_serialize_total() {
        let total = |plate: Option<&PlateSlices>| {
            let (_, events) = write_collecting_progress(plate);
            serialize_reports(&events)[0].1
        };
        assert!(total(Some(&plate_slices())) > total(None));
    }
}
