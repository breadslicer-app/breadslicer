/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The package part that stores the declared facet normals of a mesh object,
//! `/Metadata/declared-facet-normals/<object id>.bin`. It is linked from the
//! package root and marked MustPreserve, so an editor that changes the package
//! keeps it; the geometry fingerprint it holds then drops the normals of a mesh
//! that the editor changed.
//!
//! Layout, little-endian: the magic `SCFN`, the version (u32), the object id
//! (u32), the face count (u32), the geometry fingerprint (48 bytes), then the 3
//! bit patterns (u32) of each facet normal, in face order.

use std::collections::HashMap;

use sc_3mf::model::resource_ids::ResourceId;
use sc_3mf::names::MUST_PRESERVE_REL_TYPE;
use sc_3mf::package::{Package, PartName, Relationship};
use sc_mesh_core::indexed_mesh::{DeclaredFacetNormals, GeometryFingerprint, RestoreRefusal};
use sc_mesh_core::{IndexedMesh, MeshMetadata};

use crate::error::WriteThreeMfError;
use crate::messages::LoadThreeMfWarning;

const DIRECTORY: &str = "/Metadata/declared-facet-normals/";
const EXTENSION: &str = "bin";
const CONTENT_TYPE: &str = "application/vnd.soulcrafted.declared-facet-normals";
const REL_TYPE: &str = "http://schemas.soulcrafted.it/slicer/2026/10/declared-facet-normals";
const MAGIC: [u8; 4] = *b"SCFN";
const VERSION: u32 = 1;

/// Adds a declared-facet-normals part, linked from the package root and marked
/// MustPreserve, for each of `mesh_objects` whose mesh holds declared facet
/// normals that match its geometry.
pub fn add_declared_facet_normals_parts(
    package: &mut Package,
    mesh_objects: &[(ResourceId, &IndexedMesh)],
) -> Result<(), WriteThreeMfError> {
    for &(object_id, mesh) in mesh_objects {
        let Some((fingerprint, normals)) = matching_declared_normals(mesh) else {
            continue;
        };
        let object_id = object_id.get();
        let name = PartName::new(&format!("{DIRECTORY}{object_id}.{EXTENSION}"))
            .expect("a decimal object id makes a valid part name");
        let target = name.as_str().to_string();
        package.add_part(name, CONTENT_TYPE, encode(object_id, fingerprint, normals)?)?;
        package.add_root_relationship(Relationship {
            id: format!("declared-facet-normals-{object_id}"),
            rel_type: REL_TYPE.to_string(),
            target: target.clone(),
        });
        package.add_root_relationship(Relationship {
            id: format!("declared-facet-normals-{object_id}-preserve"),
            rel_type: MUST_PRESERVE_REL_TYPE.to_string(),
            target,
        });
    }
    Ok(())
}

/// Returns the fingerprint and the normals of the declared facet normals of
/// `mesh`, while they match its geometry.
fn matching_declared_normals(mesh: &IndexedMesh) -> Option<(GeometryFingerprint, &[[u32; 3]])> {
    let declared = mesh.meta().as_ref()?.declared_facet_normals.as_ref()?;
    Some((declared.fingerprint(), mesh.declared_facet_normals()?))
}

fn encode(
    object_id: u32,
    fingerprint: GeometryFingerprint,
    normals: &[[u32; 3]],
) -> Result<Vec<u8>, WriteThreeMfError> {
    let count = encode_facet_count(object_id, normals.len())?;
    let digest: [u8; 48] = fingerprint.into();
    let mut bytes = Vec::with_capacity(64 + 12 * normals.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&object_id.to_le_bytes());
    bytes.extend_from_slice(&count);
    bytes.extend_from_slice(&digest);
    for bits in normals.as_flattened() {
        bytes.extend_from_slice(&bits.to_le_bytes());
    }
    Ok(bytes)
}

fn encode_facet_count(object_id: u32, count: usize) -> Result<[u8; 4], WriteThreeMfError> {
    u32::try_from(count)
        .map(u32::to_le_bytes)
        .map_err(|_| WriteThreeMfError::FacetCountExceedsU32 { object_id, count })
}

/// Declared facet normals read from a package part, not yet checked against the
/// mesh of their object.
pub(crate) struct StoredFacetNormals {
    part: String,
    fingerprint: GeometryFingerprint,
    normals: Vec<[u32; 3]>,
}

/// Reads every part in the declared-facet-normals directory of `package`, keyed
/// by the object id that its name gives, with a warning for each part that is
/// not valid.
pub(crate) fn read_declared_facet_normals_parts(
    package: &Package,
) -> (HashMap<u32, StoredFacetNormals>, Vec<LoadThreeMfWarning>) {
    let mut stored = HashMap::new();
    let mut warnings = Vec::new();
    for name in package.part_names() {
        if !name.as_str().starts_with(DIRECTORY) {
            continue;
        }
        let bytes = package
            .part_data(name)
            .expect("iterated part names resolve to their parts");
        let decoded =
            object_id_of(name).and_then(|object_id| Some((object_id, decode(object_id, bytes)?)));
        match decoded {
            Some((object_id, (fingerprint, normals))) => {
                stored.insert(
                    object_id,
                    StoredFacetNormals {
                        part: name.to_string(),
                        fingerprint,
                        normals,
                    },
                );
            }
            None => warnings.push(LoadThreeMfWarning::DeclaredFacetNormalsMalformed {
                part: name.to_string(),
            }),
        }
    }
    (stored, warnings)
}

/// Returns the object id that the part name gives, which must be the canonical
/// decimal text of the id, so that 2 names cannot give the same id.
fn object_id_of(name: &PartName) -> Option<u32> {
    let stem = name.file_stem_in(DIRECTORY, EXTENSION)?;
    let object_id: u32 = stem.parse().ok()?;
    (object_id.to_string() == stem).then_some(object_id)
}

/// Returns the fingerprint and the normals that `bytes` store for `object_id`,
/// or `None` when the bytes do not follow the layout.
fn decode(object_id: u32, bytes: &[u8]) -> Option<(GeometryFingerprint, Vec<[u32; 3]>)> {
    let (magic, rest) = bytes.split_first_chunk::<4>()?;
    let (version, rest) = rest.split_first_chunk::<4>()?;
    let (stored_id, rest) = rest.split_first_chunk::<4>()?;
    let (count, rest) = rest.split_first_chunk::<4>()?;
    let (digest, body) = rest.split_first_chunk::<48>()?;
    let (facets, tail) = body.as_chunks::<12>();
    let valid = *magic == MAGIC
        && u32::from_le_bytes(*version) == VERSION
        && u32::from_le_bytes(*stored_id) == object_id
        && u32::from_le_bytes(*count) as usize == facets.len()
        && tail.is_empty();
    if !valid {
        return None;
    }
    let normals = facets
        .iter()
        .map(|facet| {
            let (words, _) = facet.as_chunks::<4>();
            [words[0], words[1], words[2]].map(u32::from_le_bytes)
        })
        .collect();
    Some((GeometryFingerprint::from_bytes(*digest), normals))
}

/// Binds `stored` to `mesh`, the converted mesh of object `object_id`, as its
/// declared facet normals, or returns the warning that tells why it cannot.
pub(crate) fn restore(
    mesh: &mut IndexedMesh,
    object_id: u32,
    stored: StoredFacetNormals,
) -> Result<(), LoadThreeMfWarning> {
    let StoredFacetNormals {
        part,
        fingerprint,
        normals,
    } = stored;
    let declared = DeclaredFacetNormals::restore(mesh, normals, fingerprint).map_err(
        |refusal| match refusal {
            RestoreRefusal::GeometryChanged => {
                LoadThreeMfWarning::DeclaredFacetNormalsMismatch { object_id }
            }
            RestoreRefusal::FacetCountMismatch(_) => {
                LoadThreeMfWarning::DeclaredFacetNormalsMalformed { part }
            }
        },
    )?;
    let metadata = mesh.meta().clone().unwrap_or_default();
    mesh.set_meta(Some(MeshMetadata {
        declared_facet_normals: Some(declared),
        ..metadata
    }));
    Ok(())
}

#[cfg(test)]
mod declared_normals_tests {
    use super::*;

    const FINGERPRINT: [u8; 48] = [7; 48];

    /// Returns a valid encoding for object 3 with 2 facets.
    fn valid_part() -> Vec<u8> {
        encode(
            3,
            GeometryFingerprint::from_bytes(FINGERPRINT),
            &[[1, 2, 3], [4, 5, 6]],
        )
        .expect("2 facets fit the count field")
    }

    fn part_name(name: &str) -> PartName {
        PartName::new(name).expect("a valid part name")
    }

    #[test]
    fn decode_reads_back_what_encode_wrote() {
        let (fingerprint, normals) = decode(3, &valid_part()).expect("a valid part");
        assert_eq!(fingerprint, GeometryFingerprint::from_bytes(FINGERPRINT));
        assert_eq!(normals, [[1, 2, 3], [4, 5, 6]]);
    }

    #[test]
    fn decode_refuses_a_part_that_breaks_the_layout() {
        type Change = fn(&mut Vec<u8>);
        let cases: [(&str, Change); 6] = [
            ("magic", |bytes| bytes[0] = b'X'),
            ("version", |bytes| bytes[4] = 2),
            ("object id", |bytes| bytes[8] = 4),
            ("face count", |bytes| bytes[12] = 3),
            ("a truncated facet", |bytes| {
                bytes.pop();
            }),
            ("a truncated header", |bytes| bytes.truncate(63)),
        ];
        for (case, change) in cases {
            let mut bytes = valid_part();
            change(&mut bytes);
            assert_eq!(decode(3, &bytes), None, "{case}");
        }
    }

    #[test]
    fn object_id_of_takes_the_canonical_decimal_stem() {
        assert_eq!(
            object_id_of(&part_name("/Metadata/declared-facet-normals/7.bin")),
            Some(7)
        );
        for name in [
            "/Metadata/declared-facet-normals/07.bin",
            "/Metadata/declared-facet-normals/+7.bin",
            "/Metadata/declared-facet-normals/x.bin",
            "/Metadata/declared-facet-normals/7.txt",
            "/Metadata/declared-facet-normals/a/7.bin",
            "/Metadata/declared-facet-normals/4294967296.bin",
        ] {
            assert_eq!(object_id_of(&part_name(name)), None, "{name}");
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_facet_count_above_u32_max_fails_the_write() {
        let count = u32::MAX as usize + 1;
        assert!(matches!(
            encode_facet_count(5, count),
            Err(WriteThreeMfError::FacetCountExceedsU32 { object_id: 5, count: c }) if c == count
        ));
        assert_eq!(
            encode_facet_count(5, u32::MAX as usize).ok(),
            Some([0xff; 4])
        );
    }
}
