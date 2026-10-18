/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Unit tests for the OPC package layer.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::rc::Rc;

use quick_xml::XmlVersion;
use quick_xml::errors::{Error as XmlError, IllFormedError};
use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;

use crate::error::{MalformedPartNameError, PackageReadError, PackageWriteError};
use crate::names::{
    DEFAULT_MODEL_PART_NAME, MODEL_PART_CONTENT_TYPE, START_PART_REL_TYPE, THUMBNAIL_REL_TYPE,
};
use crate::package::static_part_name::StaticPartName;
use crate::package::{DecompressionProgress, Package, PackageReadLimits, PartName, Relationship};

fn part(name: &str) -> PartName {
    PartName::new(name).expect("valid part name")
}

fn start_part_relationship(target: &str) -> Relationship {
    Relationship {
        id: "rel-1".to_string(),
        rel_type: START_PART_REL_TYPE.to_string(),
        target: target.to_string(),
    }
}

/// Returns a package with 1 model part, 1 extra part, a root StartPart
/// relationship, and 1 part-level relationship.
fn sample_package() -> Package {
    let mut package = Package::new();
    package
        .add_part(
            part(DEFAULT_MODEL_PART_NAME.as_str()),
            MODEL_PART_CONTENT_TYPE,
            b"<model/>".to_vec(),
        )
        .expect("unique name");
    package
        .add_part(
            part("/2D/slices1.model"),
            MODEL_PART_CONTENT_TYPE,
            b"<model2/>".to_vec(),
        )
        .expect("unique name");
    package
        .add_part(part("/Metadata/thumbnail.png"), "image/png", vec![1, 2, 3])
        .expect("unique name");
    package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));
    package.add_root_relationship(Relationship {
        id: "rel-2".to_string(),
        rel_type: THUMBNAIL_REL_TYPE.to_string(),
        target: "/Metadata/thumbnail.png".to_string(),
    });
    package.add_part_relationship(
        &part(DEFAULT_MODEL_PART_NAME.as_str()),
        Relationship {
            id: "rel-100".to_string(),
            rel_type: START_PART_REL_TYPE.to_string(),
            target: "/2D/slices1.model".to_string(),
        },
    );
    package
}

fn write_to_bytes(package: &Package) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    package.write_to(&mut cursor).expect("package writes");
    cursor.into_inner()
}

const MINIMAL_CONTENT_TYPES: &str = include_str!("../tests/fixtures/opc/minimal_content_types.xml");
const MINIMAL_RELS: &str = include_str!("../tests/fixtures/opc/minimal_rels.xml");

/// Returns a hand-built archive holding `content_types`, `rels` as the root
/// relationships part, and the model part `<model/>` styled by `model_options`.
fn opc_package_bytes(
    content_types: &str,
    rels: &str,
    model_options: zip::write::SimpleFileOptions,
) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    let mut archive = zip::write::ZipWriter::new(&mut cursor);
    let plain = zip::write::SimpleFileOptions::default();
    archive.start_file("[Content_Types].xml", plain).unwrap();
    archive.write_all(content_types.as_bytes()).unwrap();
    archive.start_file("_rels/.rels", plain).unwrap();
    archive.write_all(rels.as_bytes()).unwrap();
    archive
        .start_file("3D/3dmodel.model", model_options)
        .unwrap();
    archive.write_all(b"<model/>").unwrap();
    archive.finish().unwrap();
    cursor.into_inner()
}

/// Returns a hand-built archive holding the minimal fixture entries (content
/// types, root rels, `<model/>`), so tests know the exact decompressed size of
/// every entry. `model_options` styles the model entry (e.g. forcing ZIP64
/// structures via `large_file`).
fn minimal_package_bytes(model_options: zip::write::SimpleFileOptions) -> Vec<u8> {
    opc_package_bytes(MINIMAL_CONTENT_TYPES, MINIMAL_RELS, model_options)
}

/// Returns the decompressed size of every entry `minimal_package_bytes` writes.
fn minimal_package_decompressed_len() -> u64 {
    (MINIMAL_CONTENT_TYPES.len() + MINIMAL_RELS.len() + b"<model/>".len()) as u64
}

/// Returns a ZIP archive holding 1 deflated entry named `big.bin`.
fn single_entry_archive(payload: Vec<u8>) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    let mut archive = zip::write::ZipWriter::new(&mut cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    archive.start_file("big.bin", options).unwrap();
    archive.write_all(&payload).unwrap();
    archive.finish().unwrap();
    cursor.into_inner()
}

/// Forges the entry's 4-byte uncompressed-size field, in both the local file
/// header and the central directory, to claim `claimed` bytes. Single-entry
/// archives only.
fn forge_claimed_size(bytes: &mut [u8], claimed: u32) {
    assert_eq!(&bytes[0..4], b"PK\x03\x04", "local header at start");
    bytes[22..26].copy_from_slice(&claimed.to_le_bytes());
    // The last central-directory signature is the real one: the entry's payload
    // can only appear before the directory.
    let central = bytes
        .windows(4)
        .rposition(|w| w == b"PK\x01\x02")
        .expect("central directory header");
    bytes[central + 24..central + 28].copy_from_slice(&claimed.to_le_bytes());
}

/// Returns the decompressed bytes of the item `name` of a ZIP archive.
fn item_bytes(archive: &[u8], name: &str) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive)).expect("a ZIP archive");
    let mut item = archive.by_name(name).expect("the item is present");
    let mut bytes = Vec::new();
    item.read_to_end(&mut bytes).expect("the item reads");
    bytes
}

fn item_names(archive: &[u8]) -> BTreeSet<String> {
    let archive = zip::ZipArchive::new(Cursor::new(archive)).expect("a ZIP archive");
    archive.file_names().map(String::from).collect()
}

/// Returns a ZIP archive that stores `items` (name, bytes) uncompressed, in
/// that order.
fn stored_archive(items: &[(&str, &[u8])]) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    let mut archive = zip::write::ZipWriter::new(&mut cursor);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in items {
        archive.start_file(*name, stored).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();
    cursor.into_inner()
}

/// Returns a Media Types stream that holds `entries` (Default and Override
/// elements).
fn content_types_part(entries: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">{entries}</Types>"#
    )
}

/// Returns pseudo-random bytes, which DEFLATE barely compresses.
fn incompressible_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

/// A seekable in-memory writer that refuses, as a full disk does, every write
/// that would make it longer than `capacity`. `len` follows its length;
/// `lengths` keeps its length after each write.
struct ProbeWriter {
    inner: Cursor<Vec<u8>>,
    capacity: u64,
    len: Rc<Cell<u64>>,
    lengths: Vec<u64>,
}

impl ProbeWriter {
    fn with_capacity(capacity: u64) -> Self {
        Self {
            inner: Cursor::new(Vec::new()),
            capacity,
            len: Rc::default(),
            lengths: Vec::new(),
        }
    }

    fn unlimited() -> Self {
        Self::with_capacity(u64::MAX)
    }
}

impl Write for ProbeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.inner.position() + buf.len() as u64 > self.capacity {
            return Err(std::io::ErrorKind::StorageFull.into());
        }
        let n = self.inner.write(buf)?;
        let len = self.inner.get_ref().len() as u64;
        self.len.set(len);
        self.lengths.push(len);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for ProbeWriter {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// A seekable in-memory writer whose write call `fail_at` (counted from 0)
/// fails once, while the calls after it succeed, as after a transient error. It
/// counts its write calls and the writes that reach it after the failure.
struct FlakyWriter {
    inner: Cursor<Vec<u8>>,
    calls: usize,
    fail_at: usize,
    writes_after_failure: usize,
}

impl FlakyWriter {
    fn failing_at(fail_at: usize) -> Self {
        Self {
            inner: Cursor::new(Vec::new()),
            calls: 0,
            fail_at,
            writes_after_failure: 0,
        }
    }
}

impl Write for FlakyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let call = self.calls;
        self.calls += 1;
        if call == self.fail_at {
            return Err(std::io::Error::other("transient write failure"));
        }
        if call > self.fail_at {
            self.writes_after_failure += 1;
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for FlakyWriter {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[cfg(test)]
mod part_name_tests {
    use super::*;

    #[test]
    fn accepts_conventional_names() {
        for name in [
            "/3D/3dmodel.model",
            "/[Content_Types].xml",
            "/2D/8cb173a3.model",
            "/Metadata/thumbnail.png",
            "/single",
        ] {
            assert!(PartName::new(name).is_ok(), "should accept {name}");
        }
    }

    #[test]
    fn rejects_malformed_names() {
        assert_eq!(PartName::new(""), Err(MalformedPartNameError::Empty));
        assert_eq!(
            PartName::new("3D/model"),
            Err(MalformedPartNameError::NotAbsolute),
        );
        assert_eq!(
            PartName::new("/"),
            Err(MalformedPartNameError::EmptySegment)
        );
        assert_eq!(
            PartName::new("/3D//model"),
            Err(MalformedPartNameError::EmptySegment),
        );
        assert_eq!(
            PartName::new("/3D/model."),
            Err(MalformedPartNameError::TrailingDot),
        );
    }

    #[test]
    fn displays_as_its_name() {
        assert_eq!(part("/3D/3dmodel.model").to_string(), "/3D/3dmodel.model");
    }
}

#[cfg(test)]
mod static_part_name_tests {
    use super::*;

    #[test]
    fn accepts_conventional_names_and_gives_them_back() {
        for name in [
            "/3D/3dmodel.model",
            "/[Content_Types].xml",
            "/2D/8cb173a3.model",
            "/Metadata/thumbnail.png",
            "/single",
            "/3D/modèle.model",
        ] {
            assert_eq!(
                StaticPartName::new(name).map(StaticPartName::as_str),
                Ok(name),
            );
        }
    }

    #[test]
    fn rejects_malformed_names() {
        for (name, error) in [
            ("", MalformedPartNameError::Empty),
            ("3D/model", MalformedPartNameError::NotAbsolute),
            ("3D/model.", MalformedPartNameError::NotAbsolute),
            ("/", MalformedPartNameError::EmptySegment),
            ("//", MalformedPartNameError::EmptySegment),
            ("//.", MalformedPartNameError::EmptySegment),
            ("/3D//model", MalformedPartNameError::EmptySegment),
            ("/3D/", MalformedPartNameError::EmptySegment),
            ("/3D/model.", MalformedPartNameError::TrailingDot),
            ("/3D/modèle.", MalformedPartNameError::TrailingDot),
        ] {
            assert_eq!(StaticPartName::new(name), Err(error), "{name:?}");
        }
    }

    #[test]
    fn converts_into_the_part_name_that_new_gives() {
        let name = StaticPartName::new("/3D/3dmodel.model").expect("valid part name");
        assert_eq!(PartName::from(name), part("/3D/3dmodel.model"));
    }
}

#[cfg(test)]
mod part_name_file_stem_tests {
    use super::*;

    #[test]
    fn gives_the_stem_of_a_file_in_the_directory_with_the_extension() {
        let name = part("/Metadata/profiles/printers/a-b.toml");
        assert_eq!(
            name.file_stem_in("/Metadata/profiles/printers/", "toml"),
            Some("a-b"),
        );
    }

    #[test]
    fn gives_the_stem_of_a_root_level_part() {
        assert_eq!(part("/x.bin").file_stem_in("/", "bin"), Some("x"));
    }

    #[test]
    fn keeps_the_dots_inside_the_stem() {
        let name = part("/Metadata/a/x.y.toml");
        assert_eq!(name.file_stem_in("/Metadata/a/", "toml"), Some("x.y"));
    }

    #[test]
    fn refuses_a_part_outside_the_directory() {
        for (name, directory) in [
            ("/Metadata/a/b/c.toml", "/Metadata/a/"),
            ("/Metadata/b/c.toml", "/Metadata/a/"),
            ("/Metadata/a/c.toml", "/Metadata/a"),
        ] {
            assert_eq!(
                part(name).file_stem_in(directory, "toml"),
                None,
                "{name} in {directory}",
            );
        }
    }

    #[test]
    fn refuses_another_extension() {
        for name in [
            "/Metadata/a/c.json",
            "/Metadata/a/c.TOML",
            "/Metadata/a/ctoml",
        ] {
            assert_eq!(
                part(name).file_stem_in("/Metadata/a/", "toml"),
                None,
                "{name}"
            );
        }
    }

    #[test]
    fn refuses_a_file_name_without_a_stem() {
        for name in ["/Metadata/a/.toml", "/Metadata/a/toml"] {
            assert_eq!(
                part(name).file_stem_in("/Metadata/a/", "toml"),
                None,
                "{name}"
            );
        }
    }
}

#[cfg(test)]
mod round_trip_tests {
    use super::*;

    #[test]
    fn package_round_trips_parts_content_types_and_relationships() {
        let original = sample_package();
        let bytes = write_to_bytes(&original);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");

        let names: Vec<&str> = restored.part_names().map(PartName::as_str).collect();
        assert!(
            names.contains(&DEFAULT_MODEL_PART_NAME.as_str()),
            "names: {names:?}"
        );
        assert!(names.contains(&"/2D/slices1.model"));
        assert!(names.contains(&"/Metadata/thumbnail.png"));
        assert_eq!(names.len(), 3, "rels parts must not surface as parts");

        assert_eq!(
            restored.part_content_type(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(MODEL_PART_CONTENT_TYPE),
        );
        assert_eq!(
            restored.part_content_type(&part("/Metadata/thumbnail.png")),
            Some("image/png"),
        );
        assert_eq!(
            restored.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(b"<model/>".as_slice()),
        );

        assert_eq!(restored.root_relationships().len(), 2);
        let part_rels = restored.part_relationships(&part(DEFAULT_MODEL_PART_NAME.as_str()));
        assert_eq!(part_rels.len(), 1);
        assert_eq!(part_rels[0].target, "/2D/slices1.model");

        let start = restored.start_part().expect("start part resolves");
        assert_eq!(start.as_str(), DEFAULT_MODEL_PART_NAME.as_str());
    }

    #[test]
    fn conflicting_extension_content_types_round_trip_via_overrides() {
        let mut package = Package::new();
        package
            .add_part(part("/a/data.bin"), "application/x-alpha", vec![1])
            .unwrap();
        package
            .add_part(part("/b/data.bin"), "application/x-beta", vec![2])
            .unwrap();
        package.add_root_relationship(start_part_relationship("/a/data.bin"));
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            restored.part_content_type(&part("/a/data.bin")),
            Some("application/x-alpha"),
        );
        assert_eq!(
            restored.part_content_type(&part("/b/data.bin")),
            Some("application/x-beta"),
        );
    }

    #[test]
    fn relative_start_part_target_resolves_from_the_root() {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![],
            )
            .unwrap();
        package.add_root_relationship(start_part_relationship("3D/3dmodel.model"));
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            restored.start_part().expect("resolves").as_str(),
            DEFAULT_MODEL_PART_NAME.as_str(),
        );
    }
}

#[cfg(test)]
mod written_archive_tests {
    use super::*;

    const RELATIONSHIPS_MEDIA_TYPE: &str =
        "application/vnd.openxmlformats-package.relationships+xml";

    fn string_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// Returns the Default entries (extension to media type) and the Override
    /// entries (part name to media type) of a Media Types stream.
    fn media_type_entries(xml: &[u8]) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        let mut reader = NsReader::from_reader(xml);
        let mut defaults = BTreeMap::new();
        let mut overrides = BTreeMap::new();
        loop {
            match reader.read_event().expect("well-formed XML") {
                Event::Start(e) | Event::Empty(e) => {
                    let value = |key: &str| {
                        let attribute = e.try_get_attribute(key).expect("attributes").expect(key);
                        let value = attribute.normalized_value(XmlVersion::Implicit1_0);
                        value.expect("a value").into_owned()
                    };
                    match e.local_name().as_ref() {
                        b"Default" => {
                            defaults.insert(value("Extension"), value("ContentType"));
                        }
                        b"Override" => {
                            overrides.insert(value("PartName"), value("ContentType"));
                        }
                        _ => {}
                    }
                }
                Event::Eof => return (defaults, overrides),
                _ => {}
            }
        }
    }

    fn written_media_types(
        package: &Package,
    ) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        media_type_entries(&item_bytes(&write_to_bytes(package), "[Content_Types].xml"))
    }

    /// Asserts that `xml` starts with an XML 1.0 declaration that names UTF-8
    /// or UTF-16 if it names an encoding, and that its root element is `root`
    /// in `namespace`.
    fn assert_opc_xml(xml: &[u8], root: &str, namespace: &str) {
        let mut reader = NsReader::from_reader(xml);
        match reader.read_event().expect("well-formed XML") {
            Event::Decl(declaration) => {
                assert_eq!(declaration.version().expect("a version").as_ref(), b"1.0");
                if let Some(encoding) = declaration.encoding() {
                    let encoding = encoding.expect("an encoding name");
                    assert!(
                        encoding.eq_ignore_ascii_case(b"UTF-8")
                            || encoding.eq_ignore_ascii_case(b"UTF-16"),
                        "{}",
                        String::from_utf8_lossy(&encoding),
                    );
                }
            }
            other => panic!("not a declaration: {other:?}"),
        }
        loop {
            match reader.read_resolved_event().expect("well-formed XML") {
                (resolved, Event::Start(e) | Event::Empty(e)) => {
                    assert_eq!(e.local_name().as_ref(), root.as_bytes());
                    assert_eq!(
                        resolved,
                        ResolveResult::Bound(Namespace(namespace.as_bytes()))
                    );
                    return;
                }
                (_, Event::Eof) => panic!("no root element"),
                _ => {}
            }
        }
    }

    #[test]
    fn item_names_are_the_part_names_without_their_leading_slash() {
        let expected: BTreeSet<String> = [
            "2D/slices1.model",
            "3D/3dmodel.model",
            "3D/_rels/3dmodel.model.rels",
            "Metadata/thumbnail.png",
            "[Content_Types].xml",
            "_rels/.rels",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(item_names(&write_to_bytes(&sample_package())), expected);
    }

    #[test]
    fn an_extension_with_1_media_type_gets_a_default() {
        let (defaults, overrides) = written_media_types(&sample_package());
        assert_eq!(
            defaults,
            string_map(&[
                ("model", MODEL_PART_CONTENT_TYPE),
                ("png", "image/png"),
                ("rels", RELATIONSHIPS_MEDIA_TYPE),
            ]),
        );
        assert_eq!(overrides, BTreeMap::new());
    }

    #[test]
    fn an_extension_with_2_media_types_gets_overrides() {
        let mut package = Package::new();
        package
            .add_part(part("/a/data.bin"), "application/x-alpha", vec![1])
            .unwrap();
        package
            .add_part(part("/b/data.bin"), "application/x-beta", vec![2])
            .unwrap();
        let (defaults, overrides) = written_media_types(&package);
        assert_eq!(defaults, string_map(&[("rels", RELATIONSHIPS_MEDIA_TYPE)]));
        assert_eq!(
            overrides,
            string_map(&[
                ("/a/data.bin", "application/x-alpha"),
                ("/b/data.bin", "application/x-beta"),
            ]),
        );
    }

    #[test]
    fn a_part_without_an_extension_gets_an_override() {
        // The dot in the folder name is not in the last segment.
        let mut package = Package::new();
        package
            .add_part(part("/a.b/notes"), "text/plain", vec![1])
            .unwrap();
        package
            .add_part(part("/c.png"), "image/png", vec![2])
            .unwrap();
        let (defaults, overrides) = written_media_types(&package);
        assert_eq!(
            defaults,
            string_map(&[("png", "image/png"), ("rels", RELATIONSHIPS_MEDIA_TYPE)]),
        );
        assert_eq!(overrides, string_map(&[("/a.b/notes", "text/plain")]));
    }

    #[test]
    fn a_name_that_starts_with_its_only_dot_gets_the_default_of_its_extension() {
        let mut package = Package::new();
        package
            .add_part(part("/a/.hidden"), "text/plain", vec![1])
            .unwrap();
        let (defaults, overrides) = written_media_types(&package);
        assert_eq!(
            defaults,
            string_map(&[("hidden", "text/plain"), ("rels", RELATIONSHIPS_MEDIA_TYPE)]),
        );
        assert_eq!(overrides, BTreeMap::new());
    }

    #[test]
    fn written_xml_parts_declare_xml_1_0_and_use_the_opc_namespaces() {
        let bytes = write_to_bytes(&sample_package());
        let content_types = "http://schemas.openxmlformats.org/package/2006/content-types";
        let relationships = "http://schemas.openxmlformats.org/package/2006/relationships";
        assert_opc_xml(
            &item_bytes(&bytes, "[Content_Types].xml"),
            "Types",
            content_types,
        );
        assert_opc_xml(
            &item_bytes(&bytes, "_rels/.rels"),
            "Relationships",
            relationships,
        );
        assert_opc_xml(
            &item_bytes(&bytes, "3D/_rels/3dmodel.model.rels"),
            "Relationships",
            relationships,
        );
    }
}

#[cfg(test)]
mod item_mapping_tests {
    use super::*;

    fn relationships_part(body: &str) -> String {
        format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#
        )
    }

    fn part_names_of(package: &Package) -> Vec<&str> {
        package.part_names().map(PartName::as_str).collect()
    }

    #[test]
    fn an_item_name_maps_to_the_part_name_with_a_slash_added() {
        let content_types = content_types_part(
            r#"<Default Extension="bin" ContentType="application/octet-stream"/>"#,
        );
        let bytes = stored_archive(&[
            ("[Content_Types].xml", content_types.as_bytes()),
            ("x.bin", b"x"),
        ]);
        let package = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(part_names_of(&package), ["/x.bin"]);
    }

    #[test]
    fn folder_items_are_not_parts() {
        // A name that ends with a slash is a folder, whatever its data.
        let bytes = stored_archive(&[
            ("3D/", &[0; 100]),
            ("[Content_Types].xml", MINIMAL_CONTENT_TYPES.as_bytes()),
            ("_rels/.rels", MINIMAL_RELS.as_bytes()),
            ("3D/3dmodel.model", b"<model/>"),
        ]);
        let mut ticks = Vec::new();
        let package = Package::read_from_with_progress(Cursor::new(bytes), |p| ticks.push(p))
            .expect("package reads");
        assert_eq!(part_names_of(&package), [DEFAULT_MODEL_PART_NAME.as_str()]);
        let last = ticks.last().expect("reading a package ticks");
        assert_eq!(last.total_bytes, minimal_package_decompressed_len());
        assert_eq!(last.decompressed_bytes, minimal_package_decompressed_len());
    }

    #[test]
    fn only_rels_names_in_a_rels_segment_are_relationships_parts() {
        let content_types = content_types_part(
            r#"<Default Extension="txt" ContentType="text/plain"/><Override PartName="/data/notes.rels" ContentType="text/plain"/>"#,
        );
        let bytes = stored_archive(&[
            ("[Content_Types].xml", content_types.as_bytes()),
            ("_rels/notes.txt", b"plain text"),
            ("data/notes.rels", b"plain text"),
        ]);
        let package = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            part_names_of(&package),
            ["/_rels/notes.txt", "/data/notes.rels"]
        );
        assert_eq!(
            package.part_content_type(&part("/data/notes.rels")),
            Some("text/plain")
        );
    }

    #[test]
    fn an_empty_relationships_part_does_not_hide_the_next_one() {
        let content_types = content_types_part(
            r#"<Default Extension="bin" ContentType="application/octet-stream"/>"#,
        );
        let empty = relationships_part("");
        let one = relationships_part(r#"<Relationship Id="r1" Type="t" Target="/a.bin"/>"#);
        let bytes = stored_archive(&[
            ("[Content_Types].xml", content_types.as_bytes()),
            ("a.bin", b"a"),
            ("b.bin", b"b"),
            ("_rels/a.bin.rels", empty.as_bytes()),
            ("_rels/b.bin.rels", one.as_bytes()),
        ]);
        let package = Package::read_from(Cursor::new(bytes)).expect("package reads");
        let restored = Package::read_from(Cursor::new(write_to_bytes(&package)))
            .expect("written package reads");
        let expected = Relationship {
            id: "r1".to_string(),
            rel_type: "t".to_string(),
            target: "/a.bin".to_string(),
        };
        assert_eq!(restored.part_relationships(&part("/b.bin")), [expected]);
    }
}

#[cfg(test)]
mod media_type_tests {
    use super::*;

    #[test]
    fn an_override_matches_its_part_name_in_any_ascii_case() {
        let content_types = content_types_part(concat!(
            r#"<Default Extension="bin" ContentType="application/octet-stream"/>"#,
            r#"<Override PartName="/DATA/A.BIN" ContentType="text/plain"/>"#,
            r#"<Override PartName="/data/b.bin" ContentType="text/csv"/>"#,
        ));
        let bytes = stored_archive(&[
            ("[Content_Types].xml", content_types.as_bytes()),
            ("data/a.bin", b"a"),
            ("Data/B.Bin", b"b"),
        ]);
        let package = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            package.part_content_type(&part("/data/a.bin")),
            Some("text/plain")
        );
        assert_eq!(
            package.part_content_type(&part("/Data/B.Bin")),
            Some("text/csv")
        );
    }

    #[test]
    fn a_name_that_starts_with_its_only_dot_matches_the_default_of_its_extension() {
        let content_types =
            content_types_part(r#"<Default Extension="hidden" ContentType="text/plain"/>"#);
        let bytes = stored_archive(&[
            ("[Content_Types].xml", content_types.as_bytes()),
            ("a/.hidden", b"x"),
        ]);
        let package = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            package.part_content_type(&part("/a/.hidden")),
            Some("text/plain")
        );
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn duplicate_part_names_are_rejected_at_add_time() {
        let mut package = Package::new();
        package
            .add_part(part("/a.bin"), "application/octet-stream", vec![])
            .unwrap();
        let err = package
            .add_part(part("/a.bin"), "application/octet-stream", vec![])
            .unwrap_err();
        assert!(matches!(err, PackageWriteError::DuplicatePartName { name } if name == "/a.bin"),);
    }

    #[test]
    fn a_part_name_that_differs_only_in_ascii_case_is_rejected_at_add_time() {
        let mut package = Package::new();
        package
            .add_part(part("/a.bin"), "application/octet-stream", vec![])
            .unwrap();
        let err = package
            .add_part(part("/A.BIN"), "application/octet-stream", vec![])
            .unwrap_err();
        assert!(matches!(err, PackageWriteError::DuplicatePartName { name } if name == "/A.BIN"));
    }

    #[test]
    fn reserved_relationships_part_names_are_rejected_at_add_time() {
        for name in [
            "/_rels/.rels",
            "/3D/_rels/3dmodel.model.rels",
            "/data/_rels/notes.rels",
            "/data/_RELS/notes.RELS",
        ] {
            let err = Package::new()
                .add_part(part(name), "application/octet-stream", vec![])
                .unwrap_err();
            assert!(
                matches!(&err, PackageWriteError::ReservedPartName { name: refused } if refused == name),
                "{name}: {err:?}",
            );
        }
    }

    #[test]
    fn names_close_to_the_reserved_ones_are_accepted_at_add_time() {
        for name in [
            "/_rels/notes.txt",
            "/data/notes.rels",
            "/_rels",
            "/a_rels/x.rels",
        ] {
            Package::new()
                .add_part(part(name), "application/octet-stream", vec![])
                .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        }
    }

    #[test]
    fn missing_content_types_part_is_rejected() {
        let mut cursor = Cursor::new(Vec::new());
        let mut archive = zip::write::ZipWriter::new(&mut cursor);
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"<model/>").unwrap();
        archive.finish().unwrap();
        let err = Package::read_from(Cursor::new(cursor.into_inner())).unwrap_err();
        assert!(matches!(err, PackageReadError::MissingContentTypes));
    }

    #[test]
    fn missing_start_part_relationship_is_reported_on_lookup() {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![],
            )
            .unwrap();
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        let err = restored.start_part().unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::MissingStartPartRelationship
        ));
    }

    #[test]
    fn start_part_pointing_at_a_missing_part_is_reported() {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![],
            )
            .unwrap();
        package.add_root_relationship(start_part_relationship("/3D/absent.model"));
        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        let err = restored.start_part().unwrap_err();
        assert!(
            matches!(err, PackageReadError::StartPartNotFound { target } if target == "/3D/absent.model"),
        );
    }

    #[test]
    fn part_without_content_type_is_rejected() {
        // Hand-build an archive whose content-types map does not cover the
        // stored part.
        let mut cursor = Cursor::new(Vec::new());
        let mut archive = zip::write::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("[Content_Types].xml", options).unwrap();
        archive
            .write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/></Types>"#,
            )
            .unwrap();
        archive.start_file("3D/3dmodel.model", options).unwrap();
        archive.write_all(b"<model/>").unwrap();
        archive.finish().unwrap();
        let err = Package::read_from(Cursor::new(cursor.into_inner())).unwrap_err();
        assert!(
            matches!(err, PackageReadError::PartWithoutContentType { part } if part == "/3D/3dmodel.model"),
        );
    }

    #[test]
    fn content_types_without_their_end_tag_are_rejected() {
        let content_types = MINIMAL_CONTENT_TYPES.replace("</Types>", "");
        let bytes = opc_package_bytes(&content_types, MINIMAL_RELS, Default::default());
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::ContentTypesXml(XmlError::IllFormed(IllFormedError::MissingEndTag(name))) if name == "Types"),
            "{err:?}",
        );
    }

    #[test]
    fn the_innermost_open_element_is_reported() {
        let content_types = MINIMAL_CONTENT_TYPES.replace("</Types>", "<Extra>");
        let bytes = opc_package_bytes(&content_types, MINIMAL_RELS, Default::default());
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::ContentTypesXml(XmlError::IllFormed(IllFormedError::MissingEndTag(name))) if name == "Extra"),
            "{err:?}",
        );
    }

    #[test]
    fn relationships_without_their_end_tag_are_rejected() {
        let rels = MINIMAL_RELS.replace("</Relationships>", "");
        let bytes = opc_package_bytes(MINIMAL_CONTENT_TYPES, &rels, Default::default());
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::RelationshipsXml { part, source: XmlError::IllFormed(IllFormedError::MissingEndTag(name)) } if part == "/_rels/.rels" && name == "Relationships"),
            "{err:?}",
        );
    }

    #[test]
    fn content_types_cut_inside_their_root_are_rejected() {
        let start = MINIMAL_CONTENT_TYPES.find("<Types").unwrap() + 1;
        let end = MINIMAL_CONTENT_TYPES.find("</Types>").unwrap() + "</Types>".len();
        for len in start..end {
            let content_types = &MINIMAL_CONTENT_TYPES[..len];
            let bytes = opc_package_bytes(content_types, MINIMAL_RELS, Default::default());
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(err, PackageReadError::ContentTypesXml(_)),
                "{len} bytes: {err:?}",
            );
        }
    }

    #[test]
    fn relationships_cut_inside_their_root_are_rejected() {
        let start = MINIMAL_RELS.find("<Relationships").unwrap() + 1;
        let end = MINIMAL_RELS.find("</Relationships>").unwrap() + "</Relationships>".len();
        for len in start..end {
            let rels = &MINIMAL_RELS[..len];
            let bytes = opc_package_bytes(MINIMAL_CONTENT_TYPES, rels, Default::default());
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(&err, PackageReadError::RelationshipsXml { part, .. } if part == "/_rels/.rels"),
                "{len} bytes: {err:?}",
            );
        }
    }

    /// Returns the offset of the last central directory header of `bytes`.
    fn last_central_header(bytes: &[u8]) -> usize {
        bytes
            .windows(4)
            .rposition(|w| w == b"PK\x01\x02")
            .expect("central directory header")
    }

    #[test]
    fn a_start_part_target_that_is_not_a_part_name_is_reported() {
        let mut package = Package::new();
        package.add_root_relationship(start_part_relationship("/3D//a.model"));
        let err = package.start_part().unwrap_err();
        assert!(
            matches!(&err, PackageReadError::MalformedPartName { name, source: MalformedPartNameError::EmptySegment } if name == "/3D//a.model"),
            "{err:?}",
        );
    }

    #[test]
    fn bytes_that_are_not_a_zip_archive_are_rejected() {
        let err = Package::read_from(Cursor::new(MINIMAL_CONTENT_TYPES.as_bytes())).unwrap_err();
        assert!(matches!(err, PackageReadError::Zip(_)), "{err:?}");
    }

    #[test]
    fn a_damaged_local_header_is_rejected() {
        let mut bytes = minimal_package_bytes(Default::default());
        assert_eq!(&bytes[0..4], b"PK\x03\x04", "local header at start");
        bytes[0] = b'X';
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(matches!(err, PackageReadError::Zip(_)), "{err:?}");
    }

    #[test]
    fn an_encrypted_item_is_rejected() {
        // Bit 0 of the general purpose flag, in the local header and in the
        // central directory.
        let mut bytes = single_entry_archive(b"0123456789".to_vec());
        let central = last_central_header(&bytes);
        bytes[6] |= 1;
        bytes[central + 8] |= 1;
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(
                err,
                PackageReadError::Zip(zip::result::ZipError::UnsupportedArchive(_))
            ),
            "{err:?}",
        );
    }

    #[test]
    fn an_item_whose_crc_does_not_match_is_rejected() {
        let mut bytes = single_entry_archive(b"0123456789".to_vec());
        let central = last_central_header(&bytes);
        bytes[14] ^= 0xff;
        bytes[central + 16] ^= 0xff;
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::PartRead { part, .. } if part == "/big.bin"),
            "{err:?}",
        );
    }

    #[test]
    fn an_item_name_with_an_empty_segment_is_rejected() {
        let content_types =
            content_types_part(r#"<Default Extension="model" ContentType="text/plain"/>"#);
        let bytes = stored_archive(&[
            ("[Content_Types].xml", content_types.as_bytes()),
            ("3D//a.model", b"<model/>"),
        ]);
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::MalformedPartName { name, source: MalformedPartNameError::EmptySegment } if name == "/3D//a.model"),
            "{err:?}",
        );
    }

    #[test]
    fn an_item_name_with_a_leading_slash_is_rejected() {
        // OPC adds a `/` to an item name, so `/a.bin` gives `//a.bin`.
        let content_types = content_types_part(
            r#"<Default Extension="bin" ContentType="application/octet-stream"/>"#,
        );
        for names in [&["/a.bin"][..], &["a.bin", "/a.bin"]] {
            let mut items = vec![("[Content_Types].xml", content_types.as_bytes())];
            items.extend(names.iter().map(|name| (*name, &b"x"[..])));
            let err = Package::read_from(Cursor::new(stored_archive(&items))).unwrap_err();
            assert!(
                matches!(&err, PackageReadError::MalformedPartName { name, source: MalformedPartNameError::EmptySegment } if name == "//a.bin"),
                "{names:?}: {err:?}",
            );
        }
    }

    #[test]
    fn a_part_without_an_extension_or_an_override_is_rejected() {
        let content_types =
            content_types_part(r#"<Default Extension="txt" ContentType="text/plain"/>"#);
        let bytes = stored_archive(&[
            ("[Content_Types].xml", content_types.as_bytes()),
            ("LICENSE", b"text"),
        ]);
        let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
        assert!(
            matches!(&err, PackageReadError::PartWithoutContentType { part } if part == "/LICENSE"),
            "{err:?}",
        );
    }

    #[test]
    fn media_type_entries_without_a_required_attribute_are_rejected() {
        for (entry, missing) in [
            (r#"<Default ContentType="text/plain"/>"#, "Extension"),
            (r#"<Default Extension="txt"/>"#, "ContentType"),
            (r#"<Override ContentType="text/plain"/>"#, "PartName"),
            (r#"<Override PartName="/a.txt"/>"#, "ContentType"),
        ] {
            let content_types = content_types_part(entry);
            let bytes = stored_archive(&[("[Content_Types].xml", content_types.as_bytes())]);
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(err, PackageReadError::ContentTypeMissingAttribute { attribute } if attribute == missing),
                "{entry}: {err:?}",
            );
        }
    }

    #[test]
    fn a_2nd_default_for_an_extension_is_rejected() {
        for (entries, repeated) in [
            (
                r#"<Default Extension="png" ContentType="image/png"/><Default Extension="png" ContentType="image/png"/>"#,
                "png",
            ),
            (
                r#"<Default Extension="png" ContentType="image/png"/><Default Extension="PNG" ContentType="image/jpeg"/>"#,
                "PNG",
            ),
        ] {
            let content_types = content_types_part(entries);
            let bytes = stored_archive(&[("[Content_Types].xml", content_types.as_bytes())]);
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(&err, PackageReadError::DuplicateDefaultExtension { extension } if extension == repeated),
                "{entries}: {err:?}",
            );
        }
    }

    #[test]
    fn a_2nd_override_for_a_part_is_rejected() {
        for (entries, repeated) in [
            (
                r#"<Override PartName="/a.bin" ContentType="text/plain"/><Override PartName="/a.bin" ContentType="text/plain"/>"#,
                "/a.bin",
            ),
            (
                r#"<Override PartName="/a.bin" ContentType="text/plain"/><Override PartName="/A.BIN" ContentType="text/csv"/>"#,
                "/A.BIN",
            ),
        ] {
            let content_types = content_types_part(entries);
            let bytes = stored_archive(&[("[Content_Types].xml", content_types.as_bytes())]);
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(&err, PackageReadError::DuplicateOverridePartName { part_name } if part_name == repeated),
                "{entries}: {err:?}",
            );
        }
    }

    #[test]
    fn items_whose_names_opc_treats_as_1_are_rejected() {
        let content_types = content_types_part(
            r#"<Default Extension="bin" ContentType="application/octet-stream"/>"#,
        );
        // The items that follow `[Content_Types].xml`, and the 2 names that the
        // error must give.
        for (names, first, second) in [
            (&["a.bin", "A.BIN"][..], "a.bin", "A.BIN"),
            (
                &["_rels/.rels", "_RELS/.rels"],
                "_rels/.rels",
                "_RELS/.rels",
            ),
            (
                &["[content_types].xml"],
                "[Content_Types].xml",
                "[content_types].xml",
            ),
        ] {
            let mut items = vec![("[Content_Types].xml", content_types.as_bytes())];
            items.extend(names.iter().map(|name| (*name, &b"x"[..])));
            let err = Package::read_from(Cursor::new(stored_archive(&items))).unwrap_err();
            assert!(
                matches!(&err, PackageReadError::EquivalentItemNames { first: a, second: b } if a == first && b == second),
                "{names:?}: {err:?}",
            );
        }
    }

    #[test]
    fn relationships_without_a_required_attribute_are_rejected() {
        for (attributes, missing) in [
            (r#"Type="t" Target="/a.model""#, "Id"),
            (r#"Id="r1" Target="/a.model""#, "Type"),
            (r#"Id="r1" Type="t""#, "Target"),
        ] {
            let rels = format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship {attributes}/></Relationships>"#
            );
            let bytes = opc_package_bytes(MINIMAL_CONTENT_TYPES, &rels, Default::default());
            let err = Package::read_from(Cursor::new(bytes)).unwrap_err();
            assert!(
                matches!(&err, PackageReadError::RelationshipMissingAttribute { part, attribute } if part == "/_rels/.rels" && *attribute == missing),
                "{attributes}: {err:?}",
            );
        }
    }
}

#[cfg(test)]
mod read_limits_tests {
    use super::*;

    /// Returns a serialized package whose only user part is the model part,
    /// carrying `data`. The content-types and rels entries it also contains
    /// stay well under a kilobyte each.
    fn package_bytes_with_model_part(data: Vec<u8>) -> Vec<u8> {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                data,
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));
        write_to_bytes(&package)
    }

    #[test]
    fn default_limits_are_2_gib_per_part_and_4_gib_total() {
        let limits = PackageReadLimits::default();
        assert_eq!(limits.max_part_bytes, 2 * (1u64 << 30));
        assert_eq!(limits.max_total_bytes, 4 * (1u64 << 30));
    }

    #[test]
    fn part_exactly_at_the_part_cap_is_read_intact() {
        let bytes = package_bytes_with_model_part(vec![b'a'; 1024]);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let package = Package::read_from_with_limits(Cursor::new(bytes), limits)
            .expect("a part at the cap must pass");
        let data = package
            .part_data(&part(DEFAULT_MODEL_PART_NAME.as_str()))
            .expect("part present");
        assert_eq!(data.len(), 1024);
        assert!(data.iter().all(|&b| b == b'a'));
    }

    #[test]
    fn part_one_byte_over_the_part_cap_is_refused() {
        let bytes = package_bytes_with_model_part(vec![b'a'; 1025]);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, cap_bytes: 1024 }
                if part == DEFAULT_MODEL_PART_NAME.as_str()
        ));
    }

    #[test]
    fn highly_compressible_oversized_part_is_refused() {
        // 1 MiB of zeros deflates to about a kilobyte: the archive stays tiny
        // on disk while its decompressed size dwarfs the cap, so an on-disk
        // size gate alone would not catch it.
        let bytes = package_bytes_with_model_part(vec![0u8; 1 << 20]);
        assert!(bytes.len() < 64 * 1024, "archive must stay small on disk");
        let limits = PackageReadLimits {
            max_part_bytes: 64 * 1024,
            max_total_bytes: 1 << 30,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, .. } if part == DEFAULT_MODEL_PART_NAME.as_str()
        ));
    }

    #[test]
    fn parts_summing_past_the_total_cap_are_refused() {
        let mut package = Package::new();
        for name in ["/a.bin", "/b.bin", "/c.bin"] {
            package
                .add_part(part(name), "application/octet-stream", vec![b'x'; 4096])
                .expect("unique name");
        }
        package.add_root_relationship(start_part_relationship("/a.bin"));
        let bytes = write_to_bytes(&package);
        // Each part fits the per-part cap; their sum (plus the small
        // content-types and rels entries) crosses the total cap.
        let limits = PackageReadLimits {
            max_part_bytes: 8192,
            max_total_bytes: 10_000,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPackage { cap_bytes: 10_000 }
        ));
    }

    #[test]
    fn forged_header_claiming_huge_size_is_refused() {
        // A tiny entry whose forged header claims 1 GiB: the claimed-size check
        // must refuse it before allocating.
        let mut bytes = single_entry_archive(b"0123456789".to_vec());
        forge_claimed_size(&mut bytes, 1 << 30);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, cap_bytes: 1024 } if part == "/big.bin"
        ));
    }

    #[test]
    fn forged_header_claiming_fewer_bytes_than_real_is_still_capped() {
        // The header claims 10 bytes, so the claimed-size check passes; the
        // bytes actually inflated must still be counted and refused through the
        // take bound.
        let mut bytes = single_entry_archive(vec![0u8; 100 * 1024]);
        forge_claimed_size(&mut bytes, 10);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1 << 20,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPart { part, cap_bytes: 1024 } if part == "/big.bin"
        ));
    }

    #[test]
    fn forged_header_cannot_sneak_past_the_total_cap_either() {
        // Same lying-low header, but here the total ceiling is the tighter one,
        // so the overrun must surface as a package-level refusal.
        let mut bytes = single_entry_archive(vec![0u8; 100 * 1024]);
        forge_claimed_size(&mut bytes, 10);
        let limits = PackageReadLimits {
            max_part_bytes: 2048,
            max_total_bytes: 1024,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(matches!(
            err,
            PackageReadError::OversizedPackage { cap_bytes: 1024 }
        ));
    }

    #[test]
    fn a_package_exactly_at_the_total_cap_is_read() {
        let bytes = minimal_package_bytes(Default::default());
        let limits = PackageReadLimits {
            max_part_bytes: 1 << 20,
            max_total_bytes: minimal_package_decompressed_len(),
        };
        Package::read_from_with_limits(Cursor::new(bytes), limits)
            .expect("a package at the cap must pass");
    }

    #[test]
    fn a_claim_past_the_remaining_total_is_refused_before_the_read() {
        // The claim is over the total cap only; the real data is under both.
        let mut bytes = single_entry_archive(b"0123456789".to_vec());
        forge_claimed_size(&mut bytes, 4096);
        let limits = PackageReadLimits {
            max_part_bytes: 1 << 20,
            max_total_bytes: 1024,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(
            matches!(err, PackageReadError::OversizedPackage { cap_bytes: 1024 }),
            "{err:?}",
        );
    }

    #[test]
    fn the_total_cap_is_reported_when_it_is_1_byte_under_the_part_cap() {
        // The read stops 1 byte over the total cap, so it proves only that the
        // total cap is exceeded.
        let mut bytes = single_entry_archive(vec![0u8; 100 * 1024]);
        forge_claimed_size(&mut bytes, 10);
        let limits = PackageReadLimits {
            max_part_bytes: 1024,
            max_total_bytes: 1023,
        };
        let err = Package::read_from_with_limits(Cursor::new(bytes), limits).unwrap_err();
        assert!(
            matches!(err, PackageReadError::OversizedPackage { cap_bytes: 1023 }),
            "{err:?}",
        );
    }
}

#[cfg(test)]
mod zip64_tests {
    use super::*;

    /// Returns the byte offset right after a local file header's name, where
    /// the entry's extra field begins.
    fn local_extra_field_offset(bytes: &[u8], name: &[u8]) -> usize {
        let name_pos = (30..=bytes.len() - name.len())
            .find(|&p| &bytes[p - 30..p - 26] == b"PK\x03\x04" && &bytes[p..p + name.len()] == name)
            .expect("local file header for the entry");
        name_pos + name.len()
    }

    #[test]
    fn zip64_entries_are_read_intact() {
        // The writer's large_file option forces ZIP64 structures onto the entry
        // even though its real size is small, which pins ZIP64 reading without
        // a multi-gigabyte fixture.
        let zip64 = zip::write::SimpleFileOptions::default().large_file(true);
        let bytes = minimal_package_bytes(zip64);

        // The model entry's local header must carry the ZIP64
        // extended-information extra field (id 0x0001); without it this test
        // would no longer pin ZIP64 reading.
        let extra = local_extra_field_offset(&bytes, b"3D/3dmodel.model");
        assert_eq!(
            &bytes[extra..extra + 2],
            [0x01u8, 0x00],
            "zip64 extra field id"
        );

        let package = Package::read_from(Cursor::new(bytes)).expect("zip64 package reads");
        assert_eq!(
            package.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(b"<model/>".as_slice()),
        );
        assert_eq!(
            package.part_content_type(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(MODEL_PART_CONTENT_TYPE),
        );
        let start = package.start_part().expect("start part resolves");
        assert_eq!(start.as_str(), DEFAULT_MODEL_PART_NAME.as_str());
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    fn read_collecting_ticks(
        bytes: Vec<u8>,
    ) -> (
        Result<Package, PackageReadError>,
        Vec<DecompressionProgress>,
    ) {
        let mut ticks = Vec::new();
        let result = Package::read_from_with_progress(Cursor::new(bytes), |p| ticks.push(p));
        (result, ticks)
    }

    /// Asserts the ticks of an archive whose announced sizes are accurate: at
    /// least 1 tick, a fixed total on every tick, strictly growing counts
    /// (ticks only fire when bytes were inflated), and a final count equal to
    /// the total, which it returns.
    fn assert_monotone_ending_at_total(ticks: &[DecompressionProgress]) -> u64 {
        assert!(!ticks.is_empty(), "reading a package must tick");
        let total = ticks[0].total_bytes;
        for pair in ticks.windows(2) {
            assert_eq!(pair[1].total_bytes, total, "total must stay fixed");
            assert!(
                pair[1].decompressed_bytes > pair[0].decompressed_bytes,
                "ticks must carry strictly growing counts"
            );
        }
        let last = ticks.last().expect("non-empty").decompressed_bytes;
        assert_eq!(last, total, "accurate announced sizes end at the total");
        total
    }

    #[test]
    fn known_entry_sizes_produce_an_exact_total() {
        let bytes = minimal_package_bytes(zip::write::SimpleFileOptions::default());
        let (result, ticks) = read_collecting_ticks(bytes);
        result.expect("package reads");
        let total = assert_monotone_ending_at_total(&ticks);
        assert_eq!(total, minimal_package_decompressed_len());
    }

    #[test]
    fn a_1_byte_part_ends_the_ticks_at_the_total() {
        let bytes = stored_archive(&[
            ("[Content_Types].xml", MINIMAL_CONTENT_TYPES.as_bytes()),
            ("_rels/.rels", MINIMAL_RELS.as_bytes()),
            ("3D/3dmodel.model", b"x"),
        ]);
        let (result, ticks) = read_collecting_ticks(bytes);
        result.expect("package reads");
        let total = assert_monotone_ending_at_total(&ticks);
        assert_eq!(
            total,
            (MINIMAL_CONTENT_TYPES.len() + MINIMAL_RELS.len() + 1) as u64
        );
    }

    #[test]
    fn zip64_archive_ticks_end_at_its_exact_total() {
        let zip64 = zip::write::SimpleFileOptions::default().large_file(true);
        let bytes = minimal_package_bytes(zip64);
        let (result, ticks) = read_collecting_ticks(bytes);
        result.expect("zip64 package reads");
        let total = assert_monotone_ending_at_total(&ticks);
        assert_eq!(total, minimal_package_decompressed_len());
    }

    #[test]
    fn round_tripped_package_ticks_cover_parts_and_generated_entries() {
        let bytes = write_to_bytes(&sample_package());
        let (result, ticks) = read_collecting_ticks(bytes);
        result.expect("package reads");
        let total = assert_monotone_ending_at_total(&ticks);
        // The 3 payloads of `sample_package` plus the generated content-types
        // and relationship entries: the exact XML sizes are the writer's
        // business, but the total must cover at least the payload bytes.
        let payload_bytes = (b"<model/>".len() + b"<model2/>".len() + 3) as u64;
        assert!(total > payload_bytes, "total covers the non-part entries");
    }

    #[test]
    fn oversized_claim_is_refused_before_any_tick() {
        // 3 GiB claimed against the default 2 GiB per-part ceiling: the
        // claimed-size check refuses the entry before a single byte is
        // inflated, so no tick may fire.
        let mut bytes = single_entry_archive(b"0123456789".to_vec());
        forge_claimed_size(&mut bytes, 3 << 30);
        let (result, ticks) = read_collecting_ticks(bytes);
        assert!(matches!(
            result.unwrap_err(),
            PackageReadError::OversizedPart { part, .. } if part == "/big.bin"
        ));
        assert!(ticks.is_empty(), "a refused claim must not tick");
    }

    #[test]
    fn lying_low_claim_ticks_past_the_announced_total() {
        // The header claims 10 bytes but inflation yields 100 KiB; under the
        // default ceilings the entry itself is accepted, so the counts step
        // past the claimed total exactly as the `DecompressionProgress` docs
        // warn. The read still fails later (no content-types entry), which must
        // not retract the ticks already delivered.
        let mut bytes = single_entry_archive(vec![0u8; 100 * 1024]);
        forge_claimed_size(&mut bytes, 10);
        let (result, ticks) = read_collecting_ticks(bytes);
        assert!(matches!(
            result.unwrap_err(),
            PackageReadError::MissingContentTypes
        ));
        let last = ticks.last().expect("extraction must have ticked");
        assert_eq!(last.total_bytes, 10, "the forged claim is the total");
        assert_eq!(
            last.decompressed_bytes,
            100 * 1024,
            "counts report real inflated bytes, not the claim"
        );
    }
}

mod write_progress_tests {
    use super::*;

    fn write_collecting_ticks(package: &Package) -> (Vec<u8>, Vec<(u64, u64)>) {
        let mut cursor = Cursor::new(Vec::new());
        let mut ticks = Vec::new();
        package
            .write_to_with_progress(&mut cursor, |done, total| ticks.push((done, total)))
            .expect("package writes");
        (cursor.into_inner(), ticks)
    }

    /// Returns a package whose single part is larger than the compression
    /// chunk, so writing it yields several intermediate ticks.
    fn large_part_package(part_bytes: usize) -> Package {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![b'x'; part_bytes],
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));
        package
    }

    #[test]
    fn ticks_are_monotone_and_end_at_the_part_total() {
        let part_bytes = 700 * 1024;
        let (_, ticks) = write_collecting_ticks(&large_part_package(part_bytes));

        assert!(ticks.len() >= 3, "a multi-chunk part ticks several times");
        let total = ticks[0].1;
        assert_eq!(total, part_bytes as u64, "total is the sum of part bytes");
        for pair in ticks.windows(2) {
            assert_eq!(pair[1].1, total, "total stays fixed");
            assert!(pair[1].0 > pair[0].0, "counts grow strictly");
        }
        assert_eq!(
            ticks.last().unwrap().0,
            total,
            "the last tick reaches total"
        );
    }

    #[test]
    fn the_total_sums_every_part_and_ignores_generated_entries() {
        // 3 parts of 8, 9, and 3 bytes; the content-type and relationship
        // entries the writer generates are not counted.
        let (_, ticks) = write_collecting_ticks(&sample_package());
        let total = ticks[0].1;
        assert_eq!(total, (b"<model/>".len() + b"<model2/>".len() + 3) as u64);
        assert_eq!(ticks.last().unwrap().0, total);
    }

    #[test]
    fn progress_does_not_change_the_written_bytes() {
        let package = sample_package();
        let plain = write_to_bytes(&package);
        let (ticked, _) = write_collecting_ticks(&package);
        assert_eq!(plain, ticked, "reporting progress must not alter output");
    }

    #[test]
    fn an_empty_package_reports_a_zero_total() {
        let package = Package::new();
        let (_, ticks) = write_collecting_ticks(&package);
        // No content parts: nothing to tick against.
        assert!(ticks.is_empty(), "a package with no parts does not tick");
    }

    #[test]
    fn a_part_compressed_across_threads_still_ticks_to_total() {
        // 5 MiB is past the threshold that routes a part to the parallel
        // deflater, so this exercises the progress reporting of that path
        // rather than the single-threaded one.
        let part_bytes = 5 * 1024 * 1024;
        let (_, ticks) = write_collecting_ticks(&large_part_package(part_bytes));

        assert!(ticks.len() >= 3, "a large part ticks several times");
        let total = ticks[0].1;
        assert_eq!(total, part_bytes as u64, "total is the sum of part bytes");
        for pair in ticks.windows(2) {
            assert_eq!(pair[1].1, total, "total stays fixed");
            assert!(pair[1].0 > pair[0].0, "counts grow strictly");
        }
        assert_eq!(
            ticks.last().unwrap().0,
            total,
            "the last tick reaches total"
        );
    }

    /// Returns, for each tick of a write of a package with 1 part that holds
    /// `data`, the count of the tick and the length of the writer at it.
    fn ticks_and_lengths(data: Vec<u8>) -> Vec<(u64, u64)> {
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                data,
            )
            .expect("unique name");
        let mut writer = ProbeWriter::unlimited();
        let len = Rc::clone(&writer.len);
        let mut ticks = Vec::new();
        package
            .write_to_with_progress(&mut writer, |done, _| ticks.push((done, len.get())))
            .expect("package writes");
        ticks
    }

    fn steps(ticks: &[(u64, u64)]) -> Vec<u64> {
        let mut previous = 0;
        ticks
            .iter()
            .map(|&(done, _)| {
                let step = done - previous;
                previous = done;
                step
            })
            .collect()
    }

    #[test]
    fn parts_from_4_mib_tick_after_their_splice_in_the_same_steps() {
        let below = ticks_and_lengths(incompressible_bytes(4 * 1024 * 1024 - 1));
        let at = ticks_and_lengths(incompressible_bytes(4 * 1024 * 1024));
        // The writing thread feeds the deflater, and pseudo-random bytes leave
        // it at about the rate they enter.
        let grows = below.last().unwrap().1 > below[0].1;
        assert!(grows, "under 4 MiB, the writer grows between the ticks");
        let still = at.iter().all(|&(_, len)| len == at[0].1);
        assert!(
            still,
            "at 4 MiB, the whole stream is written before the ticks"
        );
        let (below, at) = (steps(&below), steps(&at));
        let full_steps = below.len().min(at.len()) - 1;
        assert_eq!(below[..full_steps], at[..full_steps]);
    }
}

#[cfg(test)]
mod write_failure_tests {
    use super::*;

    /// Asserts that a write of `package` gives `Err`, and does not panic, when
    /// the writer is full at any length that a full write goes through.
    fn assert_every_full_writer_is_reported(package: &Package) {
        let mut writer = ProbeWriter::unlimited();
        package.write_to(&mut writer).expect("package writes");
        let full_length = writer.len.get();
        let capacities: BTreeSet<u64> = std::iter::once(0)
            .chain(writer.lengths)
            .filter(|&length| length != full_length)
            .collect();
        for capacity in capacities {
            let result = package.write_to(ProbeWriter::with_capacity(capacity));
            assert!(result.is_err(), "capacity {capacity}");
        }
    }

    /// Returns `sample_package` with a part that reaches the writer while it is
    /// fed: the deflater keeps about 64 KiB of pseudo-random input.
    fn package_with_a_large_part() -> Package {
        let mut package = sample_package();
        package
            .add_part(
                part("/Metadata/noise.bin"),
                "application/octet-stream",
                incompressible_bytes(128 * 1024),
            )
            .unwrap();
        package
    }

    #[test]
    fn a_full_writer_is_reported() {
        assert_every_full_writer_is_reported(&package_with_a_large_part());
    }

    #[test]
    fn a_write_that_fails_once_is_reported_and_ends_the_writing() {
        let package = package_with_a_large_part();
        let mut writer = FlakyWriter::failing_at(usize::MAX);
        package.write_to(&mut writer).expect("package writes");
        for fail_at in 0..writer.calls {
            let mut writer = FlakyWriter::failing_at(fail_at);
            let error = package.write_to(&mut writer).unwrap_err();
            let cause = match &error {
                PackageWriteError::Io(cause) => Some(cause),
                PackageWriteError::Zip(zip::result::ZipError::Io(cause)) => Some(cause),
                _ => None,
            };
            assert_eq!(
                cause.map(ToString::to_string).as_deref(),
                Some("transient write failure"),
                "failure at write call {fail_at}: {error:?}"
            );
            assert_eq!(
                writer.writes_after_failure, 0,
                "writes after the failure at call {fail_at}"
            );
        }
    }

    #[test]
    fn a_full_writer_while_large_relationships_parts_are_written_is_reported() {
        // Each relationships part is large enough to reach the writer while it
        // is fed.
        let source = part("/a.bin");
        let mut package = Package::new();
        package
            .add_part(source.clone(), "application/octet-stream", vec![1])
            .unwrap();
        for (i, name) in incompressible_bytes(32 * 1200).chunks(32).enumerate() {
            let name: String = name.iter().map(|b| format!("{b:02x}")).collect();
            let relationship = Relationship {
                id: format!("rel{i}"),
                rel_type: "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dtexture"
                    .to_string(),
                target: format!("/3D/Textures/{name}.png"),
            };
            package.add_root_relationship(relationship.clone());
            package.add_part_relationship(&source, relationship);
        }
        assert_every_full_writer_is_reported(&package);
    }

    #[test]
    fn a_full_writer_while_a_part_is_spliced_is_reported() {
        // A part of 4 MiB is compressed across threads and spliced in before
        // its first tick, so a writer that is full 1 byte before the length at
        // that tick refuses the last write of the splice.
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                vec![b'x'; 4 * 1024 * 1024],
            )
            .unwrap();
        let mut writer = ProbeWriter::unlimited();
        let len = Rc::clone(&writer.len);
        let mut len_at_first_tick = None;
        package
            .write_to_with_progress(&mut writer, |_, _| {
                len_at_first_tick.get_or_insert(len.get());
            })
            .expect("package writes");
        let capacity = len_at_first_tick.expect("the part ticks") - 1;
        let result = package.write_to(ProbeWriter::with_capacity(capacity));
        assert!(result.is_err(), "capacity {capacity}");
    }
}

mod parallel_compression_tests {
    use super::*;

    /// Returns bytes with local structure, so the compressor finds cross-chunk
    /// back-references (exercising the dictionary priming) instead of the
    /// degenerate all-same case.
    fn varied_bytes(len: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(len + 64);
        let mut state = 0x2545_f491u32;
        while data.len() < len {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            data.extend_from_slice(
                format!(
                    "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                    state % 1000,
                    (state >> 7) % 1000,
                    (state >> 14) % 1000,
                )
                .as_bytes(),
            );
        }
        data.truncate(len);
        data
    }

    #[test]
    fn a_large_part_round_trips_through_the_parallel_path() {
        // 5 MiB is past PARALLEL_PART_THRESHOLD, so the big part is deflated
        // across threads and spliced in, while the small part stays on the
        // single-threaded path. Both must read back exact.
        let big = varied_bytes(5 * 1024 * 1024);
        let small = b"<model/>".to_vec();

        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                big.clone(),
            )
            .expect("unique name");
        package
            .add_part(
                part("/2D/slices1.model"),
                MODEL_PART_CONTENT_TYPE,
                small.clone(),
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));

        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");

        assert_eq!(
            restored.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(big.as_slice()),
        );
        assert_eq!(
            restored.part_data(&part("/2D/slices1.model")),
            Some(small.as_slice()),
        );
        assert_eq!(
            restored.part_content_type(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(MODEL_PART_CONTENT_TYPE),
        );
    }

    #[test]
    fn a_maximally_compressible_large_part_round_trips() {
        // A degenerate, all-identical part still round-trips: dictionary
        // priming across chunk boundaries must not corrupt the stream.
        let big = vec![b'x'; 5 * 1024 * 1024];
        let mut package = Package::new();
        package
            .add_part(
                part(DEFAULT_MODEL_PART_NAME.as_str()),
                MODEL_PART_CONTENT_TYPE,
                big.clone(),
            )
            .expect("unique name");
        package.add_root_relationship(start_part_relationship(DEFAULT_MODEL_PART_NAME.as_str()));

        let bytes = write_to_bytes(&package);
        let restored = Package::read_from(Cursor::new(bytes)).expect("package reads");
        assert_eq!(
            restored.part_data(&part(DEFAULT_MODEL_PART_NAME.as_str())),
            Some(big.as_slice()),
        );
    }
}
