/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Serialization phase of a 3MF write, with progress in XML bytes.

use std::fmt;
use std::io::Write;

use crate::error::ModelWriteError;

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
    pub fn write(
        self,
        progress: impl FnMut(SerializationProgress),
    ) -> Result<[Vec<u8>; N], ModelWriteError> {
        let mut sink = CountingSink {
            xml: Vec::new(),
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
    /// XML bytes of the documents already serialized.
    finished_bytes: u64,
    last_reported: u64,
    estimated_total_bytes: u64,
    progress: F,
}

impl<F: FnMut(SerializationProgress)> CountingSink<F> {
    /// Serializes `document`, reports its end, and returns its XML.
    fn serialize(&mut self, document: &ModelDocument) -> Result<Vec<u8>, ModelWriteError> {
        write_model(document, self)?;
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
