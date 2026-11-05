/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Serialization phase of a 3MF write, with progress in XML bytes.

use std::fmt;
use std::io::Write;

use crate::error::ModelWriteError;
use crate::package::MAX_PART_BYTES;

use super::slice::SliceStackContent;
use super::types::{ModelDocument, ObjectPayload};
use super::write::write_model;

/// Least distance, in serialized bytes, between 2 reports inside a document.
const REPORT_STEP: u64 = 256 * 1024;

/// Cumulative XML byte counts reported while model documents are serialized.
///
/// `estimated_total_bytes` comes from the documents before the first byte is
/// written and stays fixed for the whole phase. `serialized_bytes` can end
/// above or below it, so a consumer that derives a fraction should clamp it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SerializationProgress {
    /// XML bytes written so far, summed across the documents.
    pub serialized_bytes: u64,
    /// Estimated XML size of all the documents.
    pub estimated_total_bytes: u64,
}

/// The serialization phase of a 3MF write: the model documents of 1 package,
/// turned into XML with 1 byte count across all of them.
pub struct ModelSerialization<'d, const N: usize> {
    documents: [&'d ModelDocument; N],
    estimated_total_bytes: u64,
}

impl<'d, const N: usize> ModelSerialization<'d, N> {
    /// Prepares the serialization of `documents` and estimates their XML size.
    ///
    /// A package always holds its start part, so a call with no document does
    /// not build:
    ///
    /// ```compile_fail
    /// sc_3mf::model::serialization::ModelSerialization::new([]);
    /// ```
    pub fn new(documents: [&'d ModelDocument; N]) -> Self {
        const { assert!(N > 0, "a serialization phase needs at least 1 document") };
        let estimated_total_bytes = documents
            .iter()
            .map(|document| estimate_model_bytes(document))
            .sum();
        Self {
            documents,
            estimated_total_bytes,
        }
    }

    /// Returns the estimated XML size of the documents: the total that every
    /// report of [`Self::write`] carries.
    pub fn estimated_total_bytes(&self) -> u64 {
        self.estimated_total_bytes
    }

    /// Serializes each document as the XML payload of a 3D Model part, and
    /// returns their XML in the order of the documents.
    ///
    /// Reports the byte count, carried across the documents, after each step of
    /// at least 256 KiB and at the end of each document. Stops at the first
    /// document that fails, and returns its error.
    ///
    /// XML past [`MAX_PART_BYTES`] gives [`ModelWriteError::PartTooLarge`]: a
    /// part that large could not be read back. A buffer the system refuses
    /// gives an error too, instead of ending the process.
    pub fn write(
        self,
        progress: impl FnMut(SerializationProgress),
    ) -> Result<[Vec<u8>; N], ModelWriteError> {
        self.write_capped(MAX_PART_BYTES, progress)
    }

    /// Writes as [`Self::write`] does, under a ceiling of `cap_bytes` of XML
    /// per document.
    fn write_capped(
        self,
        cap_bytes: u64,
        progress: impl FnMut(SerializationProgress),
    ) -> Result<[Vec<u8>; N], ModelWriteError> {
        let mut sink = CountingSink {
            xml: Vec::new(),
            cap_bytes,
            over_cap: false,
            finished_bytes: 0,
            last_reported: 0,
            estimated_total_bytes: self.estimated_total_bytes,
            progress,
        };
        let mut failure = None;
        // After a failure the rest are skipped; their empty buffers are never
        // returned.
        let xml = self.documents.map(|document| {
            if failure.is_some() {
                return Vec::new();
            }
            match sink.serialize(document) {
                Ok(xml) => xml,
                Err(error) => {
                    failure = Some(error);
                    Vec::new()
                }
            }
        });
        match failure {
            Some(error) => Err(error),
            None => Ok(xml),
        }
    }
}

// Prints the document count instead of the documents, which can take megabytes.
impl<const N: usize> fmt::Debug for ModelSerialization<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelSerialization")
            .field("documents", &N)
            .field("estimated_total_bytes", &self.estimated_total_bytes)
            .finish()
    }
}

/// A `Write` sink that holds the XML of 1 document at a time and reports the
/// byte count of the whole phase.
struct CountingSink<F> {
    xml: Vec<u8>,
    /// The most XML that 1 document may take.
    cap_bytes: u64,
    /// Whether a write was refused for going past `cap_bytes`.
    over_cap: bool,
    /// XML bytes of the documents already serialized.
    finished_bytes: u64,
    last_reported: u64,
    estimated_total_bytes: u64,
    progress: F,
}

impl<F: FnMut(SerializationProgress)> CountingSink<F> {
    /// Serializes `document`, reports its end, and returns its XML.
    fn serialize(&mut self, document: &ModelDocument) -> Result<Vec<u8>, ModelWriteError> {
        if let Err(error) = write_model(document, self) {
            return Err(if self.over_cap {
                ModelWriteError::PartTooLarge {
                    cap_bytes: self.cap_bytes,
                }
            } else {
                error
            });
        }
        self.report();
        let xml = std::mem::take(&mut self.xml);
        self.finished_bytes += xml.len() as u64;
        Ok(xml)
    }

    fn serialized_bytes(&self) -> u64 {
        self.finished_bytes + self.xml.len() as u64
    }

    fn report(&mut self) {
        self.last_reported = self.serialized_bytes();
        (self.progress)(SerializationProgress {
            serialized_bytes: self.last_reported,
            estimated_total_bytes: self.estimated_total_bytes,
        });
    }
}

impl<F: FnMut(SerializationProgress)> Write for CountingSink<F> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if (self.xml.len() + buf.len()) as u64 > self.cap_bytes {
            self.over_cap = true;
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.xml
            .try_reserve(buf.len())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
        self.xml.extend_from_slice(buf);
        if self.serialized_bytes() - self.last_reported >= REPORT_STEP {
            self.report();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Returns a rough estimate of the XML size that `write_model` gives for
/// `document`.
fn estimate_model_bytes(document: &ModelDocument) -> u64 {
    // Byte guesses for 1 element of each kind, markup and numbers included.
    const BYTES_PER_VERTEX: u64 = 64;
    const BYTES_PER_TRIANGLE: u64 = 48;
    const BYTES_PER_SLICE_VERTEX: u64 = 40;
    const BYTES_PER_SLICE_SEGMENT: u64 = 24;

    let mut bytes = 0u64;
    for object in &document.objects {
        if let ObjectPayload::Mesh(mesh) = &object.payload {
            bytes += mesh.vertices.len() as u64 * BYTES_PER_VERTEX;
            bytes += mesh.triangles.len() as u64 * BYTES_PER_TRIANGLE;
        }
    }
    for stack in &document.slice_stacks {
        if let SliceStackContent::Slices(slices) = &stack.content {
            for slice in slices {
                bytes += slice.vertices.len() as u64 * BYTES_PER_SLICE_VERTEX;
                for polygon in &slice.polygons {
                    bytes += polygon.segments.len() as u64 * BYTES_PER_SLICE_SEGMENT;
                }
            }
        }
    }
    bytes
}

#[cfg(test)]
mod capped_write_tests {
    use super::*;
    use crate::model::resource_ids::ResourceId;
    use crate::model::{MeshData, MeshResolution, ObjectResource, ObjectType, Vertex3};

    fn document_of(vertices: usize) -> ModelDocument {
        let vertex = Vertex3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        ModelDocument {
            objects: vec![ObjectResource {
                id: ResourceId::MIN,
                object_type: ObjectType::Model,
                name: None,
                partnumber: None,
                property_ref: None,
                slice_stack: None,
                mesh_resolution: MeshResolution::default(),
                payload: ObjectPayload::Mesh(MeshData {
                    vertices: vec![vertex; vertices],
                    triangles: Vec::new(),
                }),
            }],
            ..ModelDocument::default()
        }
    }

    #[test]
    fn xml_past_the_cap_is_refused() {
        let document = document_of(100);
        let result = ModelSerialization::new([&document]).write_capped(1024, |_| {});
        assert!(
            matches!(
                result,
                Err(ModelWriteError::PartTooLarge { cap_bytes: 1024 })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn xml_under_the_cap_is_the_plain_serialization() {
        let document = document_of(4);
        let [capped] = ModelSerialization::new([&document])
            .write_capped(1 << 20, |_| {})
            .expect("the document fits");
        let [plain] = ModelSerialization::new([&document])
            .write(|_| {})
            .expect("the document fits");
        assert_eq!(capped, plain);
    }
}
