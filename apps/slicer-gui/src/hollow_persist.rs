/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Models hollowed in the Hollow tab, kept in a `.bread` project (#161).
//!
//! The document holds the hollows made here ([`SceneDocument::hollows`]):
//! per hollowed leaf, the mesh it was hollowed from, its drains and the
//! settings. A save stores each one whose leaf still shows its hollow as a
//! [`HollowDocument`]: the original goes in the file as one more mesh (#211),
//! and the leaf keeps the hollow mesh, so a reader that does not know these
//! documents opens the model hollowed. On opening, the load
//! worker reads them back against the leaves' meshes, and the window puts
//! them in the document once the scene is installed: the review then knows
//! the hollow (#158), and a new drain hollows the original again.
//!
//! A document of version 1 is, little-endian:
//!
//! ```text
//! f32 wall, f32 voxel, u8 fill, f32 cell, f32 beam, u8 smooth,
//! f32 drain radius                       the Hollow tab's settings
//! u32 count, per drain: 3 f32 centre, 3 f32 direction, f32 radius
//!                                        in the leaf's local frame
//! ```
//!
//! Before the `.bread` hollows chunk named the original as a mesh of the file
//! (#211), every version ended with the original mesh itself.
//!
//! `smooth` is the smoothing builds before #29 had: written 1, not read.
//!
//! Version 2 adds the drains' shapes (#316, #317, #318) and their plugs:
//!
//! ```text
//! f32 wall … f32 drain radius             as in version 1
//! u8 shape, f32 depth, f32 inner radius,
//! f32 plug clearance                      the drain tool's shape
//! u32 count, per drain: 3 f32 centre, 3 f32 direction, f32 radius,
//!   u8 shape, f32 depth, f32 inner radius, f32 plug clearance
//! u32 count, per plug: u32 drain, u32 vertices, u32 faces,
//!   u64 fingerprint                        the plug models it made
//! ```
//!
//! `shape` holds a bit per part that is set: 1 the depth, 2 the cone, 4 the
//! plug; a part not set reads its value as unused. A plug model is found
//! again on opening by its mesh ([`plug_fingerprint`]), so a new drain
//! replaces it as before saving. A hollow whose drains are all plain, with
//! the drain tool plain, is still written as version 1, which builds before
//! these shapes read.
//!
//! Version 3 adds the areas drawn to stay solid (#179), after the plugs:
//!
//! ```text
//! … as in version 2, up to the plugs
//! u32 count, per area: u8 perspective, 3 f32 origin, 3 f32 u, 3 f32 v,
//!   3 f32 w, u32 points, per point 2 f32  in the leaf's local frame
//! ```
//!
//! A hollow with no such area is written as version 1 or 2, as before.
//!
//! Version 4 adds what made the cavity (#446), after the areas, which it
//! always holds (none is a count of 0):
//!
//! ```text
//! … as in version 3, up to the areas
//! u8 set: 1 the engine, 2 the stretch
//! u32 engine                              HOLLOW_ENGINE_VERSION, 0 if unset
//! 9 f64 stretch, row by row               the model's scale, identity if unset
//! ```
//!
//! Every hollow made since is written as version 4. One read from an older
//! file, which knows neither, keeps its version when saved again.
//!
//! Version 5 adds the narrowest cavity kept (#454), after the stretch:
//!
//! ```text
//! … as in version 4
//! f32 min width                           0 keeps every channel
//! ```
//!
//! A hollow with a minimum width is written as version 5; without (0, and
//! every hollow read from an older file, made before it), as before.
//!
//! Version 6 marks a model that came hollow (#447): `set` may hold 4, "the
//! drains were drilled into the model as it came", whose `original` is that
//! model before them, and the min width is always there (0 for none). Only
//! such a hollow is written as version 6.
//!
//! Version 7 lets an area reach only so deep (#700, a box kept solid in a
//! scene from another slicer): as version 6, then per area a `u8` (1: it
//! has a depth) and, when set, 2 `f32`, the range along `w` it reaches. Only
//! a hollow with such an area is written as version 7.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_spatial::SceneSpatial;
use sc_project_formats::HollowDocument;
use sc_scene::node::NodeKind;
use sc_scene::{NodeId, Scene};

use sc_geom::hollow::hollow_engine::LassoRegion;

use crate::hollow_run::{HollowOptions, Hollowed, LocalDrain, PlugLink};
use crate::scene_document::SceneDocument;

/// The hollower that writes these documents.
pub(crate) const HOLLOWER: &str = "breadslicer";

/// The hollower name projects saved before 2026-11-21 carry.
const EARLIER_HOLLOWER: &str = "df";

/// The newest document version this build writes and reads.
pub(crate) const HOLLOW_DOCUMENT_VERSION: u32 = 7;

/// The hollows to save with the scene of `spatial`: each hollow made here
/// that a leaf still shows, once, with the first leaf that shows it. Copies
/// of a hollowed model (Fill bed, #356) show the same mesh, which the file
/// keeps once and reopens shared, so the one document covers them all.
pub(crate) fn hollow_documents(doc: &SceneDocument, spatial: &SceneSpatial) -> Vec<HollowDocument> {
    let hollows = doc.hollows();
    let hollows = hollows.borrow();
    let scene = spatial.graph();
    let mut saved: Vec<*const IndexedMesh> = Vec::new();
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter_map(|leaf| {
            let mesh = scene.leaf_mesh_arc(leaf)?;
            let hollowed = hollows.iter().find(|h| Arc::ptr_eq(&h.hollow, mesh))?;
            if saved.contains(&Arc::as_ptr(mesh)) {
                return None;
            }
            saved.push(Arc::as_ptr(mesh));
            let (version, document) = encode(hollowed);
            Some(HollowDocument {
                leaf,
                original: hollowed.original.clone(),
                hollower: HOLLOWER.to_owned(),
                version,
                document,
            })
        })
        .collect()
}

/// What reading a file's hollows produced.
#[derive(Debug, Default)]
pub(crate) struct RestoredHollows {
    /// The hollows, each with the mesh its leaf shows.
    pub(crate) hollows: Vec<Hollowed>,
    /// Why some could not be read, one line each. Those models open as
    /// they were saved, hollow, but the Hollow tab does not know them.
    pub(crate) failures: Vec<String>,
}

/// Reads `documents` against the leaves of `scene`, off the UI thread.
pub(crate) fn restore_hollows(scene: &Scene, documents: Vec<HollowDocument>) -> RestoredHollows {
    let mut restored = RestoredHollows::default();
    for document in documents {
        match restore(scene, &document) {
            Ok(hollowed) => restored.hollows.push(hollowed),
            Err(why) => restored.failures.push(why),
        }
    }
    restored
}

fn restore(scene: &Scene, document: &HollowDocument) -> Result<Hollowed, String> {
    if document.hollower != HOLLOWER && document.hollower != EARLIER_HOLLOWER {
        return Err(format!(
            "a hollow was made by an unknown hollower, \"{}\"",
            document.hollower
        ));
    }
    if document.version > HOLLOW_DOCUMENT_VERSION {
        return Err(format!(
            "a hollow was saved by a newer Breadslicer (version {})",
            document.version
        ));
    }
    let hollow = leaf_mesh(scene, document.leaf)
        .ok_or_else(|| "a hollow belongs to a part that is not a model".to_owned())?;
    let decoded = decode(&document.document, document.version)
        .ok_or_else(|| "a hollow's data is malformed".to_owned())?;
    let plugs = decoded
        .plugs
        .iter()
        .filter_map(|&(drain, fingerprint)| {
            Some(PlugLink {
                drain,
                mesh: plug_model(scene, fingerprint)?,
            })
        })
        .collect();
    Ok(Hollowed {
        hollow,
        original: document.original.clone(),
        drains: decoded.drains,
        options: decoded.options,
        plugs,
        solids: decoded.solids,
        engine: decoded.engine,
        stretch: decoded.stretch,
        drilled: decoded.drilled,
    })
}

/// What tells a plug's mesh apart: its vertex and face counts and a hash
/// of its vertices and faces.
type Fingerprint = (u32, u32, u64);

/// A mesh's [`Fingerprint`].
pub(crate) fn plug_fingerprint(mesh: &IndexedMesh) -> Fingerprint {
    let mix = |h: u64, w: u32| (h ^ u64::from(w)).wrapping_mul(0x0100_0000_01b3);
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in mesh.vertices() {
        for c in v.as_array() {
            h = mix(h, c.to_bits());
        }
    }
    for f in mesh.faces() {
        for v in f.vertices {
            h = mix(h, u32::from(v));
        }
    }
    (mesh.vertices().len() as u32, mesh.faces().len() as u32, h)
}

/// The mesh of the model of `scene` that has `fingerprint`, if any.
fn plug_model(scene: &Scene, fingerprint: Fingerprint) -> Option<Arc<IndexedMesh>> {
    let mut stack: Vec<NodeId> = scene.roots().to_vec();
    while let Some(id) = stack.pop() {
        match &scene.node(id)?.kind {
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                stack.extend(children.iter().copied());
            }
            NodeKind::Leaf(_) => {
                let Some(mesh) = scene.leaf_mesh_arc(id) else {
                    continue;
                };
                if mesh.vertices().len() as u32 == fingerprint.0
                    && mesh.faces().len() as u32 == fingerprint.1
                    && plug_fingerprint(mesh) == fingerprint
                {
                    return Some(mesh.clone());
                }
            }
            NodeKind::Reference { .. } => {}
        }
    }
    None
}

fn leaf_mesh(scene: &Scene, leaf: NodeId) -> Option<Arc<IndexedMesh>> {
    scene.leaf_mesh_arc(leaf).cloned()
}

/// Makes `hollows`, a reopened file's, the hollows of the scene `doc` has
/// just installed.
pub(crate) fn install_hollows(doc: &SceneDocument, hollows: Vec<Hollowed>) {
    *doc.hollows().borrow_mut() = hollows;
}

/// Whether `hollowed` needs version 2: a drain or the drain tool shaped, or
/// a plug made.
fn needs_shapes(hollowed: &Hollowed) -> bool {
    let o = &hollowed.options;
    let plain = HollowOptions::default();
    let tool_plain = !o.drain_depth_set
        && !o.drain_cone
        && !o.drain_plug
        && o.drain_depth_mm == plain.drain_depth_mm
        && o.drain_inner_radius_mm == plain.drain_inner_radius_mm
        && o.plug_clearance_mm == plain.plug_clearance_mm;
    let drains_plain = hollowed.drains.iter().all(|d| {
        d.depth_mm.is_none() && d.inner_radius_mm.is_none() && d.plug_clearance_mm.is_none()
    });
    !(tool_plain && drains_plain && hollowed.plugs.is_empty())
}

/// The document of `hollowed`, and its version.
fn encode(hollowed: &Hollowed) -> (u32, Vec<u8>) {
    let drilled = hollowed.drilled;
    let depths = hollowed.solids.iter().any(|s| s.depth.is_some());
    let width = depths || drilled || hollowed.options.min_width_mm > 0.0;
    let record = width || hollowed.engine.is_some() || hollowed.stretch.is_some();
    let solids = record || !hollowed.solids.is_empty();
    let shapes = solids || needs_shapes(hollowed);
    let mut out = Vec::new();
    let f32s = |out: &mut Vec<u8>, values: &[f32]| {
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    };
    let shape = |out: &mut Vec<u8>, set: [Option<f32>; 3]| {
        out.push(
            set.iter()
                .enumerate()
                .filter(|(_, v)| v.is_some())
                .fold(0u8, |bits, (k, _)| bits | 1 << k),
        );
        f32s(out, &set.map(|v| v.unwrap_or(0.0)));
    };
    let o = &hollowed.options;
    f32s(&mut out, &[o.wall_mm, o.voxel_mm]);
    out.push(o.fill);
    f32s(&mut out, &[o.cell_mm, o.beam_mm]);
    // The smoothing the tab had before #29: the cavity is the exact wall
    // now. Written on, which a build before it reads as its own default
    // (off would have it chamfer the cavity, which thins the wall).
    out.push(1);
    f32s(&mut out, &[o.drain_radius_mm]);
    if shapes {
        // The tool's values are kept even while off, so a part turned on
        // again comes back as it was.
        out.push(
            u8::from(o.drain_depth_set) | u8::from(o.drain_cone) << 1 | u8::from(o.drain_plug) << 2,
        );
        f32s(
            &mut out,
            &[
                o.drain_depth_mm,
                o.drain_inner_radius_mm,
                o.plug_clearance_mm,
            ],
        );
    }
    out.extend_from_slice(&(hollowed.drains.len() as u32).to_le_bytes());
    for d in &hollowed.drains {
        f32s(&mut out, &d.center);
        f32s(&mut out, &d.direction);
        f32s(&mut out, &[d.radius_mm]);
        if shapes {
            shape(
                &mut out,
                [d.depth_mm, d.inner_radius_mm, d.plug_clearance_mm],
            );
        }
    }
    if shapes {
        out.extend_from_slice(&(hollowed.plugs.len() as u32).to_le_bytes());
        for plug in &hollowed.plugs {
            let (vertices, faces, hash) = plug_fingerprint(&plug.mesh);
            out.extend_from_slice(&(plug.drain as u32).to_le_bytes());
            out.extend_from_slice(&vertices.to_le_bytes());
            out.extend_from_slice(&faces.to_le_bytes());
            out.extend_from_slice(&hash.to_le_bytes());
        }
    }
    if solids {
        out.extend_from_slice(&(hollowed.solids.len() as u32).to_le_bytes());
        for area in &hollowed.solids {
            out.push(u8::from(area.perspective));
            for axis in [area.origin, area.u, area.v, area.w] {
                f32s(&mut out, &axis);
            }
            out.extend_from_slice(&(area.polygon.len() as u32).to_le_bytes());
            for point in &area.polygon {
                f32s(&mut out, point);
            }
        }
    }
    if record {
        out.push(
            u8::from(hollowed.engine.is_some())
                | u8::from(hollowed.stretch.is_some()) << 1
                | u8::from(drilled) << 2,
        );
        out.extend_from_slice(&hollowed.engine.unwrap_or(0).to_le_bytes());
        let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        for v in hollowed.stretch.unwrap_or(identity) {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    if width {
        out.extend_from_slice(&hollowed.options.min_width_mm.to_le_bytes());
    }
    if depths {
        for area in &hollowed.solids {
            out.push(u8::from(area.depth.is_some()));
            if let Some(depth) = area.depth {
                f32s(&mut out, &depth);
            }
        }
    }
    let version = if depths {
        7
    } else if drilled {
        6
    } else if width {
        5
    } else if record {
        4
    } else if solids {
        3
    } else if shapes {
        2
    } else {
        1
    };
    (version, out)
}

/// Reads a document, front to back.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f32s<const N: usize>(&mut self) -> Option<[f32; N]> {
        let mut values = [0.0; N];
        for value in &mut values {
            *value = self.f32()?;
        }
        Some(values)
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// A shape: its bits, and the values the set ones hold.
    fn shape(&mut self) -> Option<[Option<f32>; 3]> {
        let bits = self.u8()?;
        if bits >> 3 != 0 {
            return None;
        }
        let values: [f32; 3] = self.f32s()?;
        Some(std::array::from_fn(|k| {
            (bits >> k & 1 == 1).then_some(values[k])
        }))
    }
}

/// A document read back.
#[derive(Debug)]
struct Decoded {
    options: HollowOptions,
    drains: Vec<LocalDrain>,
    /// Each plug: its drain and the fingerprint of its model.
    plugs: Vec<(usize, Fingerprint)>,
    solids: Vec<LassoRegion>,
    engine: Option<u32>,
    stretch: Option<[f64; 9]>,
    drilled: bool,
}

/// Reads a document of `version` (1 to 7).
fn decode(bytes: &[u8], version: u32) -> Option<Decoded> {
    let (shapes, solids, record, width) = match version {
        0 | 1 => (false, false, false, false),
        2 => (true, false, false, false),
        3 => (true, true, false, false),
        4 => (true, true, true, false),
        5..=7 => (true, true, true, true),
        _ => return None,
    };
    let mut input = Reader(bytes);
    let [wall_mm, voxel_mm] = input.f32s()?;
    let fill = input.u8()?;
    let [cell_mm, beam_mm] = input.f32s()?;
    // The smoothing of builds before #29: no setting now.
    let _smooth = input.u8()?;
    let drain_radius_mm = input.f32()?;
    let mut options = HollowOptions {
        wall_mm,
        voxel_mm,
        fill,
        cell_mm,
        beam_mm,
        drain_radius_mm,
        // Made before #454 kept channels open: as it was.
        min_width_mm: 0.0,
        ..HollowOptions::default()
    };
    if shapes {
        let tool = input.u8()?;
        if tool >> 3 != 0 {
            return None;
        }
        let [depth, inner, clearance] = input.f32s()?;
        options.drain_depth_set = tool & 1 != 0;
        options.drain_cone = tool & 2 != 0;
        options.drain_plug = tool & 4 != 0;
        options.drain_depth_mm = depth;
        options.drain_inner_radius_mm = inner;
        options.plug_clearance_mm = clearance;
    }
    let count = input.u32()? as usize;
    // Each drain takes 28 bytes (41 with its shape): a count the document
    // cannot hold is a lie.
    if count > input.0.len() / if shapes { 41 } else { 28 } {
        return None;
    }
    let drains = (0..count)
        .map(|_| {
            let mut drain = LocalDrain {
                center: input.f32s()?,
                direction: input.f32s()?,
                radius_mm: input.f32()?,
                ..LocalDrain::default()
            };
            if shapes {
                [
                    drain.depth_mm,
                    drain.inner_radius_mm,
                    drain.plug_clearance_mm,
                ] = input.shape()?;
            }
            Some(drain)
        })
        .collect::<Option<Vec<_>>>()?;
    let plugs = if shapes {
        let count = input.u32()? as usize;
        if count > input.0.len() / 20 {
            return None;
        }
        (0..count)
            .map(|_| {
                let drain = input.u32()? as usize;
                let fingerprint = (input.u32()?, input.u32()?, input.u64()?);
                (drain < drains.len()).then_some((drain, fingerprint))
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let mut solids = if solids {
        let count = input.u32()? as usize;
        // Each area takes at least 57 bytes and 3 points.
        if count > input.0.len() / 81 {
            return None;
        }
        (0..count)
            .map(|_| {
                let perspective = match input.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                let [origin, u, v, w] =
                    [input.f32s()?, input.f32s()?, input.f32s()?, input.f32s()?];
                let points = input.u32()? as usize;
                if points < 3 || points > input.0.len() / 8 {
                    return None;
                }
                let polygon = (0..points)
                    .map(|_| input.f32s())
                    .collect::<Option<Vec<[f32; 2]>>>()?;
                Some(LassoRegion {
                    perspective,
                    origin,
                    u,
                    v,
                    w,
                    polygon,
                    depth: None,
                })
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let mut drilled = false;
    let (engine, stretch) = if record {
        let set = input.u8()?;
        // Version 6 may say the model came hollow (#447).
        if set >> if version >= 6 { 3 } else { 2 } != 0 {
            return None;
        }
        drilled = set & 4 != 0;
        let engine = input.u32()?;
        let mut stretch = [0.0; 9];
        for v in &mut stretch {
            *v = input.f64()?;
            if !v.is_finite() {
                return None;
            }
        }
        (
            (set & 1 != 0).then_some(engine),
            (set & 2 != 0).then_some(stretch),
        )
    } else {
        (None, None)
    };
    if width {
        let w = input.f32()?;
        // Version 6 writes it always, 0 for none.
        let none = version >= 6 && w == 0.0;
        if !(w.is_finite() && (w > 0.0 || none)) {
            return None;
        }
        options.min_width_mm = w;
    }
    if version >= 7 {
        for area in &mut solids {
            area.depth = match input.u8()? {
                0 => None,
                1 => {
                    let d: [f32; 2] = input.f32s()?;
                    Some((d.iter().all(|v| v.is_finite()) && d[0] < d[1]).then_some(d)?)
                }
                _ => return None,
            };
        }
    }
    input.0.is_empty().then_some(Decoded {
        options,
        drains,
        plugs,
        solids,
        engine,
        stretch,
        drilled,
    })
}

#[cfg(test)]
mod hollow_persist_tests {
    use sc_geom::hollow::HOLLOW_ENGINE_VERSION;
    use sc_mesh_fixtures::unit_cube_mesh;

    use super::*;

    fn hollowed() -> Hollowed {
        Hollowed {
            hollow: Arc::new(unit_cube_mesh()),
            original: Arc::new(unit_cube_mesh()),
            drains: vec![LocalDrain {
                center: [1.0, 2.0, 3.0],
                direction: [0.0, 0.0, 1.0],
                radius_mm: 1.5,
                ..LocalDrain::default()
            }],
            options: HollowOptions {
                wall_mm: 3.1,
                fill: 2,
                // As hollows were made before #454.
                min_width_mm: 0.0,
                ..HollowOptions::default()
            },
            plugs: Vec::new(),
            solids: Vec::new(),
            engine: None,
            stretch: None,
            drilled: false,
        }
    }

    /// `hollowed` as a hollow made now records it: the engine and a model
    /// stretched along x (#446).
    fn recorded() -> Hollowed {
        Hollowed {
            engine: Some(HOLLOW_ENGINE_VERSION),
            stretch: Some([1.25, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.8]),
            ..hollowed()
        }
    }

    /// #446: the engine and the stretch are written as version 4 and read
    /// back as they were, with the drains, areas and plugs of any version.
    #[test]
    fn the_engine_and_the_stretch_read_back_as_version_4() {
        let shaped_recorded = Hollowed {
            engine: recorded().engine,
            stretch: recorded().stretch,
            ..shaped()
        };
        for base in [recorded(), shaped_recorded] {
            let (version, bytes) = encode(&base);
            assert_eq!(version, 4);
            let decoded = decode(&bytes, version).expect("it decodes");
            assert_eq!(decoded.engine, base.engine);
            assert_eq!(decoded.stretch, base.stretch);
            assert_eq!(decoded.drains, base.drains);
            assert_eq!(decoded.options, base.options);
            assert_eq!(decoded.plugs.len(), base.plugs.len());
            assert!(decoded.solids.is_empty());
            // Cut short, or read as version 3, it is malformed.
            assert!(decode(&bytes[..bytes.len() - 1], version).is_none());
            assert!(decode(&bytes, 3).is_none());
        }
        // One of the two known: the other reads back unknown.
        let engine_only = Hollowed {
            stretch: None,
            ..recorded()
        };
        let (version, bytes) = encode(&engine_only);
        let decoded = decode(&bytes, version).expect("it decodes");
        assert_eq!(
            (decoded.engine, decoded.stretch),
            (engine_only.engine, None)
        );
        // A set bit no version has is malformed.
        let mut bad = bytes.clone();
        let set = bad.len() - 4 - 72 - 1;
        bad[set] |= 4;
        assert!(decode(&bad, version).is_none());
        // A hollow read from an older file keeps its version.
        assert_eq!(encode(&hollowed()).0, 1);
    }

    /// #454: a hollow made with a narrowest cavity writes it as version 5
    /// and reads it back; one read from an older file keeps every channel.
    #[test]
    fn the_narrowest_cavity_reads_back_as_version_5() {
        for base in [hollowed(), recorded()] {
            let h = Hollowed {
                options: HollowOptions {
                    min_width_mm: 2.0,
                    ..base.options
                },
                ..base
            };
            let (version, bytes) = encode(&h);
            assert_eq!(version, 5);
            let decoded = decode(&bytes, version).expect("it decodes");
            assert_eq!(decoded.options, h.options);
            assert_eq!(decoded.engine, h.engine);
            assert_eq!(decoded.stretch, h.stretch);
            assert!(decode(&bytes[..bytes.len() - 1], version).is_none());
            assert!(decode(&bytes, 4).is_none());
        }
        let (version, bytes) = encode(&recorded());
        assert_eq!(version, 4);
        assert_eq!(decode(&bytes, version).unwrap().options.min_width_mm, 0.0);
        assert!(HollowOptions::default().min_width_mm > 0.0);
    }

    /// #447: a model that came hollow, drilled as it was, reads back as
    /// such, as version 6; no other hollow is written as version 6.
    #[test]
    fn a_hollow_drilled_as_it_came_reads_back_as_version_6() {
        for base in [hollowed(), recorded()] {
            let h = Hollowed {
                drilled: true,
                ..base
            };
            let (version, bytes) = encode(&h);
            assert_eq!(version, 6);
            let decoded = decode(&bytes, version).expect("it decodes");
            assert!(decoded.drilled);
            assert_eq!(decoded.options, h.options);
            assert_eq!(decoded.drains, h.drains);
            assert!(decode(&bytes, 5).is_none(), "version 5 has no such mark");
        }
        let (version, bytes) = encode(&recorded());
        assert_eq!(version, 4);
        assert!(!decode(&bytes, version).unwrap().drilled);
    }

    /// `hollowed` with shaped drains (#316, #317, #318), a plug and the
    /// drain tool shaped.
    fn shaped() -> Hollowed {
        let plain = hollowed();
        let base = plain.drains[0];
        Hollowed {
            drains: vec![
                base,
                LocalDrain {
                    depth_mm: Some(4.5),
                    ..base
                },
                LocalDrain {
                    center: [-2.0, 0.5, 9.0],
                    inner_radius_mm: Some(0.8),
                    plug_clearance_mm: Some(0.1),
                    ..base
                },
            ],
            options: HollowOptions {
                drain_cone: true,
                drain_inner_radius_mm: 0.7,
                drain_plug: true,
                plug_clearance_mm: 0.15,
                drain_depth_mm: 5.0,
                ..plain.options
            },
            plugs: vec![PlugLink {
                drain: 2,
                mesh: Arc::new(unit_cube_mesh()),
            }],
            ..plain
        }
    }

    #[test]
    fn a_hollow_document_reads_back_what_was_written() {
        let h = hollowed();
        let (version, bytes) = encode(&h);
        assert_eq!(version, 1, "plain drains stay readable by older builds");
        let decoded = decode(&bytes, version).expect("it decodes");
        assert_eq!(decoded.options, h.options);
        assert_eq!(decoded.drains, h.drains);
        assert!(decoded.plugs.is_empty());
    }

    /// #316–#318: the drains' depths, cones and plugs, the drain tool and
    /// the plugs made are written as version 2 and read back as they were.
    #[test]
    fn shaped_drains_read_back_as_version_2() {
        let h = shaped();
        let (version, bytes) = encode(&h);
        assert_eq!(version, 2);
        let decoded = decode(&bytes, version).expect("it decodes");
        assert_eq!(decoded.options, h.options);
        assert_eq!(decoded.drains, h.drains);
        assert_eq!(
            decoded.plugs,
            vec![(2, plug_fingerprint(&unit_cube_mesh()))]
        );
        // Read as version 1, it is malformed: never half read.
        assert!(decode(&bytes, 1).is_none());
        assert!(decode(&bytes, 3).is_none());
    }

    /// #700: an area that reaches only so deep (a box kept solid) is
    /// written as version 7 and read back as it was.
    #[test]
    fn a_solid_box_reads_back_as_version_7() {
        let square = |depth: Option<[f32; 2]>| LassoRegion {
            perspective: false,
            origin: [1.0, -2.0, 3.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            w: [0.0, 0.0, 4.0],
            polygon: vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]],
            depth,
        };
        let h = Hollowed {
            solids: vec![square(Some([-0.5, 0.5])), square(None)],
            ..hollowed()
        };
        let (version, bytes) = encode(&h);
        assert_eq!(version, 7);
        let decoded = decode(&bytes, version).expect("it decodes");
        assert_eq!(decoded.solids, h.solids);
        assert!(decode(&bytes[..bytes.len() - 1], version).is_none());
        // Read as version 6, the depths are left over: malformed.
        assert!(decode(&bytes, 6).is_none());
    }

    /// #179: the areas kept solid are written as version 3 and read back as
    /// they were; without them a hollow stays version 1 or 2.
    #[test]
    fn solid_areas_read_back_as_version_3() {
        let area = |perspective: bool, n: usize| LassoRegion {
            perspective,
            origin: [1.0, -2.0, 30.0],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 0.5, 0.0],
            w: [0.0, 0.0, -1.0],
            polygon: (0..n)
                .map(|k| {
                    let a = k as f32 / n as f32 * std::f32::consts::TAU;
                    [a.cos() * 0.3, a.sin() * 0.2]
                })
                .collect(),
            depth: None,
        };
        for base in [hollowed(), shaped()] {
            let h = Hollowed {
                solids: vec![area(true, 40), area(false, 3)],
                ..base
            };
            let (version, bytes) = encode(&h);
            assert_eq!(version, 3);
            let decoded = decode(&bytes, version).expect("it decodes");
            assert_eq!(decoded.solids, h.solids);
            assert_eq!(decoded.drains, h.drains);
            assert_eq!(decoded.options, h.options);
            assert_eq!(decoded.plugs.len(), h.plugs.len());
            // Cut short, it is malformed: never half read.
            assert!(decode(&bytes[..bytes.len() - 1], version).is_none());
            // Read as version 2, it is malformed too.
            assert!(decode(&bytes, 2).is_none());
        }
        assert_ne!(encode(&hollowed()).0, 3);
    }

    /// A version 1 document, as builds before #316 wrote it, byte for byte:
    /// it still opens, its drains plain.
    #[test]
    fn a_version_1_document_still_opens() {
        let mut bytes = Vec::new();
        for v in [3.1f32, 0.65] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.push(2);
        for v in [4.24f32, 0.35] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.push(1);
        bytes.extend_from_slice(&1.5f32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for v in [1.0f32, 2.0, 3.0, 0.0, 0.0, 1.0, 1.5] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let decoded = decode(&bytes, 1).expect("it decodes");
        assert_eq!(decoded.drains, hollowed().drains);
        assert_eq!(decoded.options, hollowed().options);
        assert_eq!(encode(&hollowed()), (1, bytes), "written the same today");
    }

    /// A 10 mm box hollowed to a 6 mm cavity, standing on the plate: walls of
    /// 2 mm, the cavity floor at z = 2.
    fn hollow_box() -> IndexedMesh {
        let cube = unit_cube_mesh();
        let scaled = |k: f32| {
            cube.vertices
                .iter()
                .map(move |v| sc_mesh_core::Vertex::new(v.coords().map(|c| c * k)))
        };
        let vertices = scaled(10.0).chain(scaled(6.0)).collect();
        let mut faces = cube.faces.clone();
        faces.extend(cube.faces.iter().map(|f| {
            let [a, b, c] = f.vertices.map(|v| usize::from(v) as u32 + 8);
            sc_mesh_core::IndexedTriangle::from_raw_indices([a, c, b])
        }));
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// A model hollowed here, saved as a `.bread` and opened again: the
    /// Hollow tab knows it again, with its settings and drains, and the
    /// review finds the hollow under its cavity (#161).
    #[test]
    fn a_hollow_survives_a_bread_save_and_open() {
        use std::path::PathBuf;
        use std::rc::Rc;

        use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
        use sc_project_formats::{LoadedContent, ProjectPayload, load_scene};
        use sc_scene::LeafPayload;

        use crate::hollow_run::HollowRun;
        use crate::scene_save::{prepare_save, write_bundle};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        use crate::viewport::ViewportController;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hollow.bread");
        let (found, kept) = on_fresh_slint_thread(move || {
            let window = crate::MainWindow::new().expect("a main window");
            // Saved: a box hollowed here, with one drain.
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let hollow = Arc::new(hollow_box());
            let mut scene = Scene::new();
            let leaf = scene
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 5.0])),
                    LeafPayload::Mesh(hollow.clone()),
                )
                .unwrap();
            scene.add_root(leaf).unwrap();
            document.install_graph(scene, PathBuf::from("hollow.stl"));
            let made = recorded();
            document.hollows().borrow_mut().push(Hollowed {
                hollow,
                ..made.clone()
            });
            let bundle = prepare_save(&document, ProjectPayload::default(), path.clone())
                .expect("a scene to save");
            write_bundle(&bundle, |_, _, _| {}).expect("the project writes");

            // Opened again, in a fresh window's document, as the loader does.
            let loaded = load_scene(&path, None, |_| {}).expect("it opens");
            let LoadedContent::Project(project) = loaded.content else {
                panic!("a .bread opens as a project");
            };
            let restored = restore_hollows(&project.scene, project.hollows);
            assert!(restored.failures.is_empty(), "{:?}", restored.failures);
            let ctrl = Rc::new(ViewportController::new_for_test());
            let reopened = ctrl.document();
            reopened.install_graph(project.scene, path.clone());
            install_hollows(&reopened, restored.hollows);
            let run = HollowRun::install(&window, reopened.clone(), ctrl.dialogs(), ctrl.clone());
            let kept = reopened.hollows().borrow().first().cloned();
            (
                run.hollow_at([0.3, -0.2, 2.05]),
                kept.map(|k| (k.options, k.drains, k.engine, k.stretch)),
            )
        });
        let (_, drains) = found.expect("the review finds the hollow");
        assert_eq!(drains, 1);
        let made = recorded();
        assert_eq!(
            kept,
            Some((made.options, made.drains, made.engine, made.stretch))
        );
    }

    /// #356: copies of a hollowed model (Fill bed) show its very mesh. The
    /// file keeps the hollow once, and opened again every copy is that
    /// hollow.
    #[test]
    fn copies_of_a_hollow_save_it_once() {
        use std::path::PathBuf;
        use std::rc::Rc;

        use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
        use sc_project_formats::{LoadedContent, ProjectPayload, load_scene};
        use sc_scene::LeafPayload;

        use crate::scene_save::{prepare_save, write_bundle};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        use crate::viewport::ViewportController;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("copies.bread");
        let (documents, shared) = on_fresh_slint_thread(move || {
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let hollow = Arc::new(hollow_box());
            let mut scene = Scene::new();
            for x in [0.0, 20.0, 40.0] {
                let leaf = scene
                    .add_leaf(
                        None,
                        Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 5.0])),
                        LeafPayload::Mesh(hollow.clone()),
                    )
                    .unwrap();
                scene.add_root(leaf).unwrap();
            }
            document.install_graph(scene, PathBuf::from("hollow.stl"));
            document.hollows().borrow_mut().push(Hollowed {
                hollow,
                ..hollowed()
            });
            let bundle = prepare_save(&document, ProjectPayload::default(), path.clone())
                .expect("a scene to save");
            write_bundle(&bundle, |_, _, _| {}).expect("the project writes");

            let loaded = load_scene(&path, None, |_| {}).expect("it opens");
            let LoadedContent::Project(project) = loaded.content else {
                panic!("a .bread opens as a project");
            };
            let documents = project.hollows.len();
            let restored = restore_hollows(&project.scene, project.hollows);
            assert!(restored.failures.is_empty(), "{:?}", restored.failures);
            let reopened = Rc::new(ViewportController::new_for_test()).document();
            reopened.install_graph(project.scene, path.clone());
            install_hollows(&reopened, restored.hollows);
            let kept = reopened.hollows().borrow()[0].hollow.clone();
            let shared = reopened
                .with_scene(|spatial| {
                    (0..)
                        .map_while(|i| spatial.leaf_id_at_index(i))
                        .filter(|&leaf| {
                            spatial
                                .graph()
                                .leaf_mesh_arc(leaf)
                                .is_some_and(|mesh| Arc::ptr_eq(mesh, &kept))
                        })
                        .count()
                })
                .unwrap_or_default();
            (documents, shared)
        });
        assert_eq!(documents, 1, "the hollow saved once");
        assert_eq!(shared, 3, "every copy is the hollow again");
    }

    #[test]
    fn a_damaged_hollow_document_is_refused() {
        for h in [hollowed(), shaped()] {
            let (version, bytes) = encode(&h);
            assert!(decode(&bytes[..bytes.len() - 1], version).is_none());
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(decode(&longer, version).is_none());
            assert!(decode(&[], version).is_none());
        }
    }

    /// #316–#318 through a `.bread`: a hollow with shaped drains and a plug
    /// model, saved and opened again, comes back with its drains as they
    /// were and knows its plug again (the reopened scene's own mesh), so a
    /// new drain replaces it instead of leaving a second one.
    #[test]
    fn shaped_drains_and_their_plug_survive_a_bread_save_and_open() {
        use std::path::PathBuf;
        use std::rc::Rc;

        use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
        use sc_project_formats::{LoadedContent, ProjectPayload, load_scene};
        use sc_scene::LeafPayload;

        use crate::scene_save::{prepare_save, write_bundle};
        use crate::test_support::slint_thread::on_fresh_slint_thread;
        use crate::viewport::ViewportController;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plug.bread");
        let made = shaped();
        let (kept, found) = on_fresh_slint_thread(move || {
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let hollow = Arc::new(hollow_box());
            let plug = made.plugs[0].mesh.clone();
            let mut scene = Scene::new();
            let leaf = scene
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 5.0])),
                    LeafPayload::Mesh(hollow.clone()),
                )
                .unwrap();
            scene.add_root(leaf).unwrap();
            let rid = scene.add_mesh_resource_arc(plug);
            let plug_leaf = scene
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 0.0, 0.5])),
                    LeafPayload::MeshResource(rid),
                )
                .unwrap();
            scene.add_root(plug_leaf).unwrap();
            document.install_graph(scene, PathBuf::from("plug.stl"));
            document.hollows().borrow_mut().push(Hollowed {
                hollow,
                ..made.clone()
            });
            let bundle = prepare_save(&document, ProjectPayload::default(), path.clone())
                .expect("a scene to save");
            write_bundle(&bundle, |_, _, _| {}).expect("the project writes");

            let loaded = load_scene(&path, None, |_| {}).expect("it opens");
            let LoadedContent::Project(project) = loaded.content else {
                panic!("a .bread opens as a project");
            };
            let restored = restore_hollows(&project.scene, project.hollows);
            assert!(restored.failures.is_empty(), "{:?}", restored.failures);
            let plug_meshes: Vec<Arc<IndexedMesh>> = project
                .scene
                .roots()
                .iter()
                .filter_map(|&id| project.scene.leaf_mesh_arc(id).cloned())
                .filter(|m| m.faces().len() == unit_cube_mesh().faces().len())
                .collect();
            let hollowed = restored.hollows.into_iter().next().expect("the hollow");
            let found = hollowed.plugs.len() == 1
                && plug_meshes
                    .iter()
                    .any(|m| Arc::ptr_eq(m, &hollowed.plugs[0].mesh));
            (
                (hollowed.options, hollowed.drains, hollowed.plugs[0].drain),
                found,
            )
        });
        let made = shaped();
        assert_eq!(kept, (made.options, made.drains, 2));
        assert!(found, "the plug is the reopened scene's own model");
    }
}
