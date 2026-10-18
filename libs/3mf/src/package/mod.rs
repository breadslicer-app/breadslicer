/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! OPC (Open Packaging Conventions) container layer: a ZIP archive holding
//! named parts with content types, plus relationship parts that link them. This
//! layer knows nothing about 3MF documents; it stores and retrieves bytes.
//!
//! Scope notes: content types support both Default (by extension) and Override
//! (by part) entries in both directions; relationship targets are resolved
//! against the source part's directory when relative. Interleaved packages and
//! ZIP64 writing are out of scope (ZIP64 reading comes with the `zip` crate).
//!
//! Reading refuses archives whose decompressed size exceeds fixed ceilings (2
//! GiB per part, 4 GiB per package) instead of exhausting memory on a crafted
//! or corrupt file; see `PackageReadLimits`.

mod fused_writer;
pub mod static_part_name;

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, Write};

use quick_xml::XmlVersion;
use quick_xml::errors::IllFormedError;
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::reader::NsReader;
use quick_xml::writer::Writer;
use zip::read::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::error::{MalformedPartNameError, PackageReadError, PackageWriteError};
use crate::names::{RELATIONSHIPS_CONTENT_TYPE, START_PART_REL_TYPE};

use fused_writer::FusedWriter;
use static_part_name::{StaticPartName, check_part_name};

/// Chunk size for feeding a part's bytes to the deflater, so a large part
/// reports progress as it compresses rather than in 1 step.
const COMPRESS_CHUNK_BYTES: usize = 256 * 1024;

/// Parts at least this large are compressed across threads (see
/// [`crate::deflate`]) instead of on the writing thread. Smaller parts (the
/// content-type map, relationship parts, modest models) stay on the
/// single-threaded path, where the parallel setup would not pay for itself.
const PARALLEL_PART_THRESHOLD: usize = 4 * 1024 * 1024;

/// Upper bound on a part compressed across threads. The spliced entry carries
/// 32-bit ZIP sizes, so a part near the 4 GiB limit stays on the
/// single-threaded path. Such a part is far larger than any real 3MF model and
/// could not be read back (the reader caps parts at 2 GiB), so this only avoids
/// a malformed header at the boundary.
const MAX_PARALLEL_PART: usize = (u32::MAX as usize) - 64 * 1024 * 1024;

/// Absolute, `/`-rooted name of a part inside the package.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PartName(String);

impl PartName {
    pub fn new(name: &str) -> Result<Self, MalformedPartNameError> {
        check_part_name(name)?;
        Ok(Self(name.to_string()))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the stem of this part's file name when the part sits directly in
    /// `directory` and its file name ends in `.` and `extension`, both matched
    /// exactly. `directory` is absolute and ends with `/`, as `/Metadata/`. An
    /// empty stem gives `None`.
    pub fn file_stem_in(&self, directory: &str, extension: &str) -> Option<&str> {
        if self.directory() != directory {
            return None;
        }
        let stem = self.0[directory.len()..]
            .strip_suffix(extension)?
            .strip_suffix('.')?;
        (!stem.is_empty()).then_some(stem)
    }

    /// Returns the name as a ZIP entry path (no leading slash).
    #[inline]
    pub(crate) fn zip_entry(&self) -> &str {
        &self.0[1..]
    }

    /// Returns whether OPC treats the 2 names as 1 name, which it does when
    /// they differ only in ASCII case.
    fn is_equivalent(&self, other: &PartName) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }

    /// Returns the extension of the final segment, lowercased, if any.
    fn extension_lowercase(&self) -> Option<String> {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "never None")]
        let file = self.0.rsplit('/').next()?;
        let (_, ext) = file.rsplit_once('.')?;
        Some(ext.to_ascii_lowercase())
    }

    /// Returns the directory prefix including the trailing slash (`"/3D/"` for
    /// `"/3D/3dmodel.model"`, `"/"` for a root-level part).
    fn directory(&self) -> &str {
        match self.0.rfind('/') {
            Some(idx) => &self.0[..=idx],
            // #[gamma::skip(literal.str_to_empty, reason = "names have a /")]
            // #[gamma::skip(literal.str_to_xyzzy, reason = "names have a /")]
            None => "/",
        }
    }

    /// Returns the name of this part's relationships part
    /// (`/3D/_rels/3dmodel.model.rels`).
    fn rels_part_name(&self) -> String {
        let dir = self.directory();
        let file = &self.0[dir.len()..];
        format!("{dir}_rels/{file}.rels")
    }
}

impl std::fmt::Display for PartName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<StaticPartName> for PartName {
    fn from(name: StaticPartName) -> Self {
        Self(name.as_str().to_string())
    }
}

/// An OPC relationship: a typed, identified link from a source (the package
/// root or a part) to a target part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relationship {
    pub id: String,
    pub rel_type: String,
    /// Target part name; absolute (`/`-rooted) or relative to the source's
    /// directory.
    pub target: String,
}

#[derive(Debug)]
struct Part {
    name: PartName,
    content_type: String,
    data: Vec<u8>,
}

/// Ceilings on the decompressed size of a package being read. Every entry
/// counts (parts, relationship parts, `[Content_Types].xml`); directories
/// contribute nothing. Since a `Package` holds all parts in memory, the total
/// ceiling is this layer's peak-memory bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PackageReadLimits {
    /// A single entry whose decompressed bytes exceed this is refused.
    pub(crate) max_part_bytes: u64,
    /// The package is refused once the decompressed bytes of all its entries
    /// together exceed this.
    pub(crate) max_total_bytes: u64,
}

impl Default for PackageReadLimits {
    fn default() -> Self {
        const GIB: u64 = 1 << 30;
        Self {
            max_part_bytes: 2 * GIB,
            max_total_bytes: 4 * GIB,
        }
    }
}

/// Cumulative decompressed-byte counts reported while a package is read.
///
/// `total_bytes` is the sum of the decompressed sizes announced in the ZIP
/// central directory and stays fixed for the whole read. Announced sizes are
/// claims: a forged entry can announce sizes that differ from what inflation
/// actually yields, and extraction stops early on refused archives, so
/// `decompressed_bytes` can end short of `total_bytes`, or step past it right
/// before a refusal. When the announced sizes are accurate, the final tick has
/// both equal. Consumers that derive a fraction should clamp it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecompressionProgress {
    /// Bytes inflated so far, summed across all file entries.
    pub decompressed_bytes: u64,
    /// Sum of the announced decompressed sizes of all file entries.
    pub total_bytes: u64,
}

/// An in-memory OPC package: ordered parts plus relationships. Lookups by part
/// name ignore ASCII case, as OPC does.
#[derive(Debug, Default)]
pub struct Package {
    parts: Vec<Part>,
    /// Package-root relationships (stored at `/_rels/.rels`).
    root_relationships: Vec<Relationship>,
    /// Per-part relationships, keyed by source part name; equivalent names
    /// share 1 key.
    part_relationships: BTreeMap<PartName, Vec<Relationship>>,
}

impl Package {
    // #[gamma::skip(fn_value.default, reason = "new is Self::default")]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a part. Part names must be unique, and OPC treats names that differ
    /// only in ASCII case as 1 name. Names of Relationships parts are refused:
    /// relationships go through [`Package::add_root_relationship`] and
    /// [`Package::add_part_relationship`].
    pub fn add_part(
        &mut self,
        name: PartName,
        content_type: &str,
        data: Vec<u8>,
    ) -> Result<(), PackageWriteError> {
        if is_reserved_for_relationships(&name) {
            return Err(PackageWriteError::ReservedPartName {
                name: name.as_str().to_string(),
            });
        }
        if self.find_part(&name).is_some() {
            return Err(PackageWriteError::DuplicatePartName {
                name: name.as_str().to_string(),
            });
        }
        if let Some((base, derived)) = self
            .parts
            .iter()
            .find_map(|p| derivable_pair(name.as_str(), p.name.as_str()))
        {
            return Err(PackageWriteError::DerivablePartNames {
                base: base.to_string(),
                derived: derived.to_string(),
            });
        }
        self.parts.push(Part {
            name,
            content_type: content_type.to_string(),
            data,
        });
        Ok(())
    }

    pub fn add_root_relationship(&mut self, relationship: Relationship) {
        self.root_relationships.push(relationship);
    }

    /// Adds a relationship from the part `source`. A Relationships part name is
    /// refused as the source: OPC allows no relationships from a Relationships
    /// part.
    pub fn add_part_relationship(
        &mut self,
        source: &PartName,
        relationship: Relationship,
    ) -> Result<(), PackageWriteError> {
        if is_reserved_for_relationships(source) {
            return Err(PackageWriteError::ReservedPartName {
                name: source.as_str().to_string(),
            });
        }
        let key = self
            .part_relationships
            .keys()
            .find(|key| key.is_equivalent(source))
            .unwrap_or(source)
            .clone();
        self.part_relationships
            .entry(key)
            .or_default()
            .push(relationship);
        Ok(())
    }

    pub fn part_names(&self) -> impl Iterator<Item = &PartName> {
        self.parts.iter().map(|p| &p.name)
    }

    pub fn part_content_type(&self, name: &PartName) -> Option<&str> {
        self.find_part(name).map(|p| p.content_type.as_str())
    }

    pub fn part_data(&self, name: &PartName) -> Option<&[u8]> {
        self.find_part(name).map(|p| p.data.as_slice())
    }

    pub fn root_relationships(&self) -> &[Relationship] {
        &self.root_relationships
    }

    pub fn part_relationships(&self, source: &PartName) -> &[Relationship] {
        self.part_relationships
            .iter()
            .find(|(key, _)| key.is_equivalent(source))
            .map(|(_, relationships)| relationships.as_slice())
            .unwrap_or(&[])
    }

    /// Resolves the StartPart relationship to the name of the primary 3D Model
    /// part, as the part spells it.
    pub fn start_part(&self) -> Result<PartName, PackageReadError> {
        let rel = self
            .root_relationships
            .iter()
            .find(|r| r.rel_type == START_PART_REL_TYPE)
            .ok_or(PackageReadError::MissingStartPartRelationship)?;
        let target = resolve_target("/", &rel.target);
        let name =
            PartName::new(&target).map_err(|source| PackageReadError::MalformedPartName {
                name: target.clone(),
                source,
            })?;
        match self.find_part(&name) {
            Some(part) => Ok(part.name.clone()),
            None => Err(PackageReadError::StartPartNotFound { target }),
        }
    }

    /// Serializes the package as a plain deflate-compressed ZIP.
    pub fn write_to<W: Write + Seek>(&self, writer: W) -> Result<(), PackageWriteError> {
        self.write_to_with_progress(writer, |_, _| {})
    }

    /// Writes as [`Package::write_to`] does, and reports compression progress.
    ///
    /// `progress` carries `(bytes_compressed, total_bytes)` where `total_bytes`
    /// is the sum of the raw byte lengths of the content parts (the small
    /// content-type and relationship entries are not counted). The count climbs
    /// monotonically and ends at `total_bytes`. Consumers that derive a
    /// fraction should clamp it, matching the read side.
    ///
    /// A part small enough to compress on the writing thread ticks as its bytes
    /// are fed to the deflater; a part large enough to be compressed across
    /// threads ticks once its stream is spliced in.
    pub fn write_to_with_progress<W: Write + Seek>(
        &self,
        writer: W,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), PackageWriteError> {
        self.ensure_no_derivable_names()?;
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut archive = ZipWriter::new(FusedWriter::new(writer));

        archive.start_file("[Content_Types].xml", options)?;
        archive.write_all(&self.content_types_xml()?)?;

        archive.start_file("_rels/.rels", options)?;
        archive.write_all(&relationships_xml(&self.root_relationships)?)?;

        for (source, relationships) in &self.part_relationships {
            // #[gamma::skip(cond.always_false, reason = "reads the same")]
            // #[gamma::skip(loop.delete_continue, reason = "reads the same")]
            if relationships.is_empty() {
                continue;
            }
            let rels_name = source.rels_part_name();
            archive.start_file(&rels_name[1..], options)?;
            archive.write_all(&relationships_xml(relationships)?)?;
        }

        let total: u64 = self.parts.iter().map(|p| p.data.len() as u64).sum();
        let mut compressed: u64 = 0;
        for part in &self.parts {
            let len = part.data.len();
            if (PARALLEL_PART_THRESHOLD..=MAX_PARALLEL_PART).contains(&len) {
                // Compress the part across threads, then splice the finished
                // stream in verbatim (no recompression).
                // #[gamma::skip(try.propagate_to_unwrap, reason = "size fits")]
                let entry = crate::deflate::parallel_entry(part.name.zip_entry(), &part.data)?;
                // #[gamma::skip(try.propagate_to_unwrap, reason = "valid ZIP")]
                let mut source = ZipArchive::new(Cursor::new(entry))?;
                archive.raw_copy_file(source.by_index(0)?)?;
                // Report the part's bytes in the same chunk steps the
                // single-threaded path uses, so the count climbs the same way
                // regardless of which path compressed the part.
                for chunk in part.data.chunks(COMPRESS_CHUNK_BYTES) {
                    compressed += chunk.len() as u64;
                    progress(compressed, total);
                }
            } else {
                archive.start_file(part.name.zip_entry(), options)?;
                // Feed each part in chunks so a large part still yields
                // intermediate progress instead of 1 jump from empty to done.
                for chunk in part.data.chunks(COMPRESS_CHUNK_BYTES) {
                    archive.write_all(chunk)?;
                    compressed += chunk.len() as u64;
                    progress(compressed, total);
                }
            }
        }

        archive.finish()?;
        Ok(())
    }

    /// Reads a package from a ZIP stream.
    ///
    /// The decompressed size of the archive is capped: 2 GiB for any single
    /// part and 4 GiB for the package as a whole. An archive exceeding a
    /// ceiling is refused with [`PackageReadError::OversizedPart`] or
    /// [`PackageReadError::OversizedPackage`].
    pub fn read_from<R: Read + Seek>(reader: R) -> Result<Self, PackageReadError> {
        Self::read_from_impl(reader, PackageReadLimits::default(), |_| {})
    }

    /// Reads as [`Package::read_from`] does, and reports decompression
    /// progress.
    ///
    /// `progress` is invoked from inside the extraction loop as entry bytes are
    /// inflated, carrying cumulative counts across the whole archive; see
    /// [`DecompressionProgress`] for what the counts can and cannot promise.
    pub fn read_from_with_progress<R: Read + Seek>(
        reader: R,
        progress: impl FnMut(DecompressionProgress),
    ) -> Result<Self, PackageReadError> {
        Self::read_from_impl(reader, PackageReadLimits::default(), progress)
    }

    /// Reads as [`Package::read_from`] does, under caller-chosen size ceilings;
    /// the public entry points always apply the defaults. Only the cap tests
    /// need to pick their own ceilings.
    #[cfg(test)]
    pub(crate) fn read_from_with_limits<R: Read + Seek>(
        reader: R,
        limits: PackageReadLimits,
    ) -> Result<Self, PackageReadError> {
        Self::read_from_impl(reader, limits, |_| {})
    }

    fn read_from_impl<R: Read + Seek>(
        reader: R,
        limits: PackageReadLimits,
        mut progress: impl FnMut(DecompressionProgress),
    ) -> Result<Self, PackageReadError> {
        let mut archive = ZipArchive::new(reader)?;
        // Central-directory pre-pass: sum the announced decompressed sizes so
        // every progress tick can carry the total. Announced sizes are claims;
        // the extraction below still enforces the real ceilings.
        let mut announced_total: u64 = 0;
        for index in 0..archive.len() {
            let file = archive.by_index_raw(index)?;
            if file.is_dir() {
                continue;
            }
            announced_total = announced_total.saturating_add(file.size());
        }
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let mut item_by_lowercase_name: BTreeMap<String, String> = BTreeMap::new();
        let mut total_bytes: u64 = 0;
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            if file.is_dir() {
                continue;
            }
            let raw_name = file.name().to_string();
            let name = format!("/{raw_name}");
            // #[gamma::skip(string.lower_to_upper, reason = "upper works too")]
            if let Some(first) =
                item_by_lowercase_name.insert(name.to_ascii_lowercase(), raw_name.clone())
            {
                return Err(PackageReadError::EquivalentItemNames {
                    first,
                    second: raw_name,
                });
            }
            let claimed_size = file.size();
            let ticking = TickingReader {
                inner: &mut file,
                base: total_bytes,
                read: 0,
                announced_total,
                progress: &mut progress,
            };
            let data = read_part_bytes(ticking, claimed_size, &name, limits, total_bytes)?;
            total_bytes += data.len() as u64;
            entries.push((name, data));
        }
        if let Some((base, derived)) = find_derivable(&item_by_lowercase_name) {
            return Err(PackageReadError::DerivableItemNames {
                base: base.clone(),
                derived: derived.clone(),
            });
        }

        let content_types_bytes = entries
            .iter()
            .find(|(name, _)| name == "/[Content_Types].xml")
            .map(|(_, data)| data.clone())
            .ok_or(PackageReadError::MissingContentTypes)?;
        let content_types = parse_content_types(&content_types_bytes)?;

        let mut package = Package::new();
        for (name, data) in entries {
            if name == "/[Content_Types].xml" {
                continue;
            }
            let part_name =
                PartName::new(&name).map_err(|source| PackageReadError::MalformedPartName {
                    name: name.clone(),
                    source,
                })?;
            if is_reserved_for_relationships(&part_name) {
                let source = relationships_source(&part_name)?;
                let relationships = parse_relationships(&data, part_name.as_str())?;
                match source {
                    None => package.root_relationships = relationships,
                    Some(source) => {
                        package.part_relationships.insert(source, relationships);
                    }
                }
                continue;
            }
            let content_type = content_types.lookup(&part_name).ok_or_else(|| {
                PackageReadError::PartWithoutContentType {
                    part: part_name.as_str().to_string(),
                }
            })?;
            package.parts.push(Part {
                content_type: content_type.to_string(),
                name: part_name,
                data,
            });
        }
        Ok(package)
    }

    /// Builds the `[Content_Types].xml` payload: Default entries for extensions
    /// whose parts all share the same content type, Override entries for
    /// everything else.
    // #[gamma::skip(try.propagate_to_unwrap, reason = "writes into a Vec")]
    fn content_types_xml(&self) -> Result<Vec<u8>, PackageWriteError> {
        // extension -> unanimous content type (None once conflicting).
        let mut defaults: BTreeMap<String, Option<String>> = BTreeMap::new();
        defaults.insert(
            "rels".to_string(),
            Some(RELATIONSHIPS_CONTENT_TYPE.to_string()),
        );
        for part in &self.parts {
            let Some(ext) = part.name.extension_lowercase() else {
                continue;
            };
            defaults
                .entry(ext)
                .and_modify(|ct| {
                    if ct.as_deref() != Some(part.content_type.as_str()) {
                        *ct = None;
                    }
                })
                .or_insert_with(|| Some(part.content_type.clone()));
        }

        let mut buffer = Vec::new();
        let mut writer = Writer::new(&mut buffer);
        // #[gamma::skip(option.some_to_none, reason = "UTF-8 is the default")]
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        let mut types = BytesStart::new("Types");
        types.push_attribute(("xmlns", crate::names::CONTENT_TYPES_NS));
        writer.write_event(Event::Start(types))?;
        for (extension, content_type) in &defaults {
            if let Some(content_type) = content_type {
                let mut e = BytesStart::new("Default");
                e.push_attribute(("Extension", extension.as_str()));
                e.push_attribute(("ContentType", content_type.as_str()));
                writer.write_event(Event::Empty(e))?;
            }
        }
        for part in &self.parts {
            let covered = part
                .name
                .extension_lowercase()
                .and_then(|ext| defaults.get(&ext).cloned())
                .flatten()
                .is_some_and(|ct| ct == part.content_type);
            if !covered {
                let mut e = BytesStart::new("Override");
                e.push_attribute(("PartName", part.name.as_str()));
                e.push_attribute(("ContentType", part.content_type.as_str()));
                writer.write_event(Event::Empty(e))?;
            }
        }
        writer.write_event(Event::End(BytesStart::new("Types").to_end()))?;
        Ok(buffer)
    }

    /// Fails when 1 of the names that `write_to` writes is derivable from
    /// another: the parts, `[Content_Types].xml`, and the Relationships parts
    /// of the package and of every source part.
    fn ensure_no_derivable_names(&self) -> Result<(), PackageWriteError> {
        let relationships_parts = self.part_relationships.keys().map(PartName::rels_part_name);
        let names: BTreeMap<String, String> = [
            "/[Content_Types].xml".to_string(),
            "/_rels/.rels".to_string(),
        ]
        .into_iter()
        .chain(relationships_parts)
        .chain(self.parts.iter().map(|p| p.name.as_str().to_string()))
        .map(|name| (name.to_ascii_lowercase(), name))
        .collect();
        match find_derivable(&names) {
            Some((base, derived)) => Err(PackageWriteError::DerivablePartNames {
                base: base.clone(),
                derived: derived.clone(),
            }),
            None => Ok(()),
        }
    }

    /// Returns the part whose name OPC treats as `name`.
    fn find_part(&self, name: &PartName) -> Option<&Part> {
        self.parts.iter().find(|p| p.name.is_equivalent(name))
    }
}

/// `Read` wrapper that reports cumulative decompressed byte counts through a
/// progress callback as an archive entry is inflated. `base` carries the bytes
/// of the entries already extracted, so 1 wrapper per entry still yields
/// archive-wide cumulative ticks.
struct TickingReader<'a, R, F> {
    inner: R,
    base: u64,
    read: u64,
    announced_total: u64,
    progress: &'a mut F,
}

impl<R: Read, F: FnMut(DecompressionProgress)> Read for TickingReader<'_, R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.read += n as u64;
            (self.progress)(DecompressionProgress {
                decompressed_bytes: self.base + self.read,
                total_bytes: self.announced_total,
            });
        }
        Ok(n)
    }
}

/// Reads an entry's decompressed bytes, refusing the package when the entry
/// would push memory use past a ceiling. The check has 2 layers: the header's
/// claimed size is tested before allocating (bounding pre-allocation when the
/// header lies high), and the bytes actually inflated are counted through a
/// `take` bound (bounding buffering when the header lies low).
fn read_part_bytes<R: Read>(
    reader: R,
    claimed_size: u64,
    part: &str,
    limits: PackageReadLimits,
    total_so_far: u64,
) -> Result<Vec<u8>, PackageReadError> {
    if claimed_size > limits.max_part_bytes {
        return Err(PackageReadError::OversizedPart {
            part: part.to_string(),
            cap_bytes: limits.max_part_bytes,
        });
    }
    let remaining_total = limits.max_total_bytes - total_so_far;
    if claimed_size > remaining_total {
        return Err(PackageReadError::OversizedPackage {
            cap_bytes: limits.max_total_bytes,
        });
    }
    let budget = limits.max_part_bytes.min(remaining_total);
    let mut data = Vec::with_capacity(claimed_size as usize);
    reader
        .take(budget.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|source| PackageReadError::PartRead {
            part: part.to_string(),
            source,
        })?;
    if data.len() as u64 > budget {
        if data.len() as u64 > limits.max_part_bytes {
            return Err(PackageReadError::OversizedPart {
                part: part.to_string(),
                cap_bytes: limits.max_part_bytes,
            });
        }
        return Err(PackageReadError::OversizedPackage {
            cap_bytes: limits.max_total_bytes,
        });
    }
    Ok(data)
}

/// The parsed content-type map of a package. Its keys are in ASCII lowercase,
/// because OPC ignores ASCII case when it matches extensions and Override part
/// names.
struct ContentTypes {
    defaults: BTreeMap<String, String>,
    overrides: BTreeMap<String, String>,
}

impl ContentTypes {
    fn lookup(&self, part: &PartName) -> Option<&str> {
        if let Some(ct) = self.overrides.get(&part.as_str().to_ascii_lowercase()) {
            return Some(ct);
        }
        let ext = part.extension_lowercase()?;
        self.defaults.get(&ext).map(String::as_str)
    }
}

fn attr_value(attr: &Attribute<'_>) -> Option<String> {
    // 3MF mandates XML 1.0; the reader never checks the declaration, so the 1.0
    // rules are assumed.
    attr.normalized_value(XmlVersion::Implicit1_0)
        .ok()
        .map(|v| v.into_owned())
}

/// The names of the elements that an event loop has opened and not yet closed,
/// innermost last.
#[derive(Default)]
struct OpenElements(Vec<Vec<u8>>);

impl OpenElements {
    fn track(&mut self, event: &Event<'_>) {
        match event {
            Event::Start(e) => self.0.push(e.name().as_ref().to_vec()),
            Event::End(_) => {
                self.0.pop();
            }
            _ => {}
        }
    }

    /// Fails while an element is still open, with the error that `quick-xml`
    /// gives for a missing end tag when it skips a subtree.
    fn ensure_closed(&self) -> Result<(), quick_xml::errors::Error> {
        match self.0.last() {
            None => Ok(()),
            Some(name) => {
                let name = String::from_utf8_lossy(name).into_owned();
                Err(IllFormedError::MissingEndTag(name).into())
            }
        }
    }
}

fn parse_content_types(bytes: &[u8]) -> Result<ContentTypes, PackageReadError> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut defaults = BTreeMap::new();
    let mut overrides = BTreeMap::new();
    let mut open = OpenElements::default();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(PackageReadError::ContentTypesXml)?;
        open.track(&event);
        match event {
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name();
                if local.as_ref() == b"Default" {
                    let mut extension = None;
                    let mut content_type = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.local_name().as_ref() {
                            b"Extension" => extension = attr_value(&attr),
                            b"ContentType" => content_type = attr_value(&attr),
                            _ => {}
                        }
                    }
                    let extension =
                        extension.ok_or(PackageReadError::ContentTypeMissingAttribute {
                            attribute: "Extension",
                        })?;
                    let content_type =
                        content_type.ok_or(PackageReadError::ContentTypeMissingAttribute {
                            attribute: "ContentType",
                        })?;
                    if defaults
                        .insert(extension.to_ascii_lowercase(), content_type)
                        .is_some()
                    {
                        return Err(PackageReadError::DuplicateDefaultExtension { extension });
                    }
                } else if local.as_ref() == b"Override" {
                    let mut part_name = None;
                    let mut content_type = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.local_name().as_ref() {
                            b"PartName" => part_name = attr_value(&attr),
                            b"ContentType" => content_type = attr_value(&attr),
                            _ => {}
                        }
                    }
                    let part_name =
                        part_name.ok_or(PackageReadError::ContentTypeMissingAttribute {
                            attribute: "PartName",
                        })?;
                    let content_type =
                        content_type.ok_or(PackageReadError::ContentTypeMissingAttribute {
                            attribute: "ContentType",
                        })?;
                    if overrides
                        .insert(part_name.to_ascii_lowercase(), content_type)
                        .is_some()
                    {
                        return Err(PackageReadError::DuplicateOverridePartName { part_name });
                    }
                }
            }
            // #[gamma::skip(loop.break_to_continue, reason = "endless loop")]
            Event::Eof => break,
            _ => {}
        }
        // #[gamma::skip(stmt.delete_call, reason = "memory only")]
        buffer.clear();
    }
    open.ensure_closed()
        .map_err(PackageReadError::ContentTypesXml)?;
    Ok(ContentTypes {
        defaults,
        overrides,
    })
}

fn parse_relationships(bytes: &[u8], part: &str) -> Result<Vec<Relationship>, PackageReadError> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut relationships = Vec::new();
    let mut open = OpenElements::default();
    let xml_error = |source| PackageReadError::RelationshipsXml {
        part: part.to_string(),
        source,
    };
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(xml_error)?;
        open.track(&event);
        match event {
            Event::Start(e) | Event::Empty(e) => {
                if e.local_name().as_ref() != b"Relationship" {
                    continue;
                }
                let mut id = None;
                let mut rel_type = None;
                let mut target = None;
                for attr in e.attributes().flatten() {
                    match attr.key.local_name().as_ref() {
                        b"Id" => id = attr_value(&attr),
                        b"Type" => rel_type = attr_value(&attr),
                        b"Target" => target = attr_value(&attr),
                        _ => {}
                    }
                }
                let missing = |attribute| PackageReadError::RelationshipMissingAttribute {
                    part: part.to_string(),
                    attribute,
                };
                relationships.push(Relationship {
                    id: id.ok_or_else(|| missing("Id"))?,
                    rel_type: rel_type.ok_or_else(|| missing("Type"))?,
                    target: target.ok_or_else(|| missing("Target"))?,
                });
            }
            // #[gamma::skip(loop.break_to_continue, reason = "endless loop")]
            Event::Eof => break,
            _ => {}
        }
        // #[gamma::skip(stmt.delete_call, reason = "memory only")]
        buffer.clear();
    }
    open.ensure_closed().map_err(xml_error)?;
    Ok(relationships)
}

// #[gamma::skip(try.propagate_to_unwrap, reason = "writes into a Vec")]
fn relationships_xml(relationships: &[Relationship]) -> Result<Vec<u8>, PackageWriteError> {
    let mut buffer = Vec::new();
    let mut writer = Writer::new(&mut buffer);
    // #[gamma::skip(option.some_to_none, reason = "UTF-8 is the default")]
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    let mut root = BytesStart::new("Relationships");
    root.push_attribute(("xmlns", crate::names::RELATIONSHIPS_NS));
    writer.write_event(Event::Start(root))?;
    for relationship in relationships {
        let mut e = BytesStart::new("Relationship");
        e.push_attribute(("Id", relationship.id.as_str()));
        e.push_attribute(("Type", relationship.rel_type.as_str()));
        e.push_attribute(("Target", relationship.target.as_str()));
        writer.write_event(Event::Empty(e))?;
    }
    writer.write_event(Event::End(BytesStart::new("Relationships").to_end()))?;
    Ok(buffer)
}

/// Returns whether OPC reserves a name for a Relationships part
/// (`.../_rels/<name>.rels`, in any ASCII case).
fn is_reserved_for_relationships(name: &PartName) -> bool {
    split_relationships_part_name(name).is_some()
}

/// Splits a name that OPC reserves for Relationships parts into the folder and
/// the file name of its source: `/3D/_rels/a.model.rels` gives `/3D` and
/// `a.model`.
fn split_relationships_part_name(name: &PartName) -> Option<(&str, &str)> {
    // #[gamma::skip(try.propagate_to_unwrap, reason = "names have a /")]
    let (folder, file) = name.as_str().rsplit_once('/')?;
    let (source_folder, rels_folder) = folder.rsplit_once('/')?;
    let (source_file, extension) = file.rsplit_once('.')?;
    (rels_folder.eq_ignore_ascii_case("_rels") && extension.eq_ignore_ascii_case("rels"))
        .then_some((source_folder, source_file))
}

/// Returns whether `name` is derivable from `base` in the OPC sense: it starts
/// with `base` and a `/`, in any ASCII case.
fn is_derivable(name: &str, base: &str) -> bool {
    let (name, base) = (name.as_bytes(), base.as_bytes());
    name.len() > base.len()
        && name[base.len()] == b'/'
        && name[..base.len()].eq_ignore_ascii_case(base)
}

/// Returns `(base, derived)` when 1 of the 2 names is derivable from the other.
fn derivable_pair<'a>(a: &'a str, b: &'a str) -> Option<(&'a str, &'a str)> {
    if is_derivable(a, b) {
        Some((b, a))
    } else if is_derivable(b, a) {
        Some((a, b))
    } else {
        None
    }
}

/// Returns the values `(base, derived)` of 2 keys of `names` where 1 is
/// derivable from the other, if any. The keys are names in ASCII lowercase.
fn find_derivable<V>(names: &BTreeMap<String, V>) -> Option<(&V, &V)> {
    names.iter().find_map(|(key, derived)| {
        key.match_indices('/')
            .find_map(|(index, _)| names.get(&key[..index]))
            .map(|base| (base, derived))
    })
}

/// Returns the source of a Relationships part: `None` for the package
/// (`/_rels/.rels`, in any ASCII case), otherwise the part name. Fails when the
/// source is not a part name, or is the name of a Relationships part.
fn relationships_source(rels: &PartName) -> Result<Option<PartName>, PackageReadError> {
    if rels.as_str().eq_ignore_ascii_case("/_rels/.rels") {
        return Ok(None);
    }
    let source = split_relationships_part_name(rels)
        .and_then(|(folder, file)| PartName::new(&format!("{folder}/{file}")).ok());
    match source {
        Some(source) if !is_reserved_for_relationships(&source) => Ok(Some(source)),
        _ => Err(PackageReadError::RelationshipsPartWithoutSource {
            part: rels.as_str().to_string(),
        }),
    }
}

/// Resolves a relationship target against its source directory. Absolute
/// targets pass through; relative ones are joined.
fn resolve_target(source_dir: &str, target: &str) -> String {
    if target.starts_with('/') {
        target.to_string()
    } else {
        format!("{source_dir}{target}")
    }
}
