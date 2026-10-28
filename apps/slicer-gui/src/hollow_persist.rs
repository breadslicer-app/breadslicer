/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Models hollowed in the Hollow tab, kept in a `.bread` project (#161).
//!
//! The document holds the hollows made here ([`SceneDocument::hollows`]):
//! per hollowed leaf, the mesh it was hollowed from, its drains and the
//! settings. A save stores each one whose leaf still shows its hollow as a
//! [`HollowDocument`]; the leaf keeps the hollow mesh, so a build that does
//! not know these documents opens the model hollowed. On opening, the load
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
//! u32 length, the original mesh           as a `.bread` mesh chunk holds it
//! ```

use std::sync::Arc;

use sc_bread_format::{decode_mesh, encode_mesh};
use sc_mesh_core::IndexedMesh;
use sc_mesh_spatial::SceneSpatial;
use sc_project_formats::HollowDocument;
use sc_scene::{NodeId, Scene};

use crate::hollow_run::{HollowOptions, Hollowed, LocalDrain};
use crate::scene_document::SceneDocument;

/// The hollower that writes these documents: the DragonFruit port.
pub(crate) const DF_HOLLOWER: &str = "df";

/// The document version this build writes and reads.
pub(crate) const HOLLOW_DOCUMENT_VERSION: u32 = 1;

/// The hollows to save with the scene of `spatial`: each hollow made here
/// that a leaf still shows.
pub(crate) fn hollow_documents(doc: &SceneDocument, spatial: &SceneSpatial) -> Vec<HollowDocument> {
    let hollows = doc.hollows();
    let hollows = hollows.borrow();
    let scene = spatial.graph();
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter_map(|leaf| {
            let mesh = scene.leaf_mesh_arc(leaf)?;
            let hollowed = hollows.iter().find(|h| Arc::ptr_eq(&h.hollow, mesh))?;
            Some(HollowDocument {
                leaf,
                hollower: DF_HOLLOWER.to_owned(),
                version: HOLLOW_DOCUMENT_VERSION,
                document: encode(hollowed),
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
    if document.hollower != DF_HOLLOWER {
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
    let (options, drains, original) =
        decode(&document.document).ok_or_else(|| "a hollow's data is malformed".to_owned())?;
    Ok(Hollowed {
        hollow,
        original: Arc::new(original),
        drains,
        options,
    })
}

fn leaf_mesh(scene: &Scene, leaf: NodeId) -> Option<Arc<IndexedMesh>> {
    scene.leaf_mesh_arc(leaf).cloned()
}

/// Makes `hollows`, a reopened file's, the hollows of the scene `doc` has
/// just installed.
pub(crate) fn install_hollows(doc: &SceneDocument, hollows: Vec<Hollowed>) {
    *doc.hollows().borrow_mut() = hollows;
}

fn encode(hollowed: &Hollowed) -> Vec<u8> {
    let mut out = Vec::new();
    let f32s = |out: &mut Vec<u8>, values: &[f32]| {
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    };
    let o = &hollowed.options;
    f32s(&mut out, &[o.wall_mm, o.voxel_mm]);
    out.push(o.fill);
    f32s(&mut out, &[o.cell_mm, o.beam_mm]);
    out.push(u8::from(o.smooth));
    f32s(&mut out, &[o.drain_radius_mm]);
    out.extend_from_slice(&(hollowed.drains.len() as u32).to_le_bytes());
    for d in &hollowed.drains {
        f32s(&mut out, &d.center);
        f32s(&mut out, &d.direction);
        f32s(&mut out, &[d.radius_mm]);
    }
    let mesh = encode_mesh(&hollowed.original);
    out.extend_from_slice(&(mesh.len() as u32).to_le_bytes());
    out.extend_from_slice(&mesh);
    out
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
}

fn decode(bytes: &[u8]) -> Option<(HollowOptions, Vec<LocalDrain>, IndexedMesh)> {
    let mut input = Reader(bytes);
    let [wall_mm, voxel_mm] = input.f32s()?;
    let fill = input.u8()?;
    let [cell_mm, beam_mm] = input.f32s()?;
    let smooth = input.u8()? != 0;
    let drain_radius_mm = input.f32()?;
    let options = HollowOptions {
        wall_mm,
        voxel_mm,
        fill,
        cell_mm,
        beam_mm,
        smooth,
        drain_radius_mm,
    };
    let count = input.u32()? as usize;
    // Each drain takes 28 bytes: a count the document cannot hold is a lie.
    if count > input.0.len() / 28 {
        return None;
    }
    let drains = (0..count)
        .map(|_| {
            Some(LocalDrain {
                center: input.f32s()?,
                direction: input.f32s()?,
                radius_mm: input.f32()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let length = input.u32()? as usize;
    let mesh = decode_mesh(input.take(length)?).ok()?;
    input.0.is_empty().then_some((options, drains, mesh))
}

#[cfg(test)]
mod hollow_persist_tests {
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
            }],
            options: HollowOptions {
                wall_mm: 3.1,
                fill: 2,
                smooth: false,
                ..HollowOptions::default()
            },
        }
    }

    #[test]
    fn a_hollow_document_reads_back_what_was_written() {
        let h = hollowed();
        let (options, drains, mesh) = decode(&encode(&h)).expect("it decodes");
        assert_eq!(options, h.options);
        assert_eq!(drains, h.drains);
        assert_eq!(mesh.faces().len(), h.original.faces().len());
        assert_eq!(mesh.vertices().len(), h.original.vertices().len());
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
            let made = hollowed();
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
                kept.map(|k| (k.options, k.drains)),
            )
        });
        let (_, drains) = found.expect("the review finds the hollow");
        assert_eq!(drains, 1);
        let made = hollowed();
        assert_eq!(kept, Some((made.options, made.drains)));
    }

    #[test]
    fn a_damaged_hollow_document_is_refused() {
        let bytes = encode(&hollowed());
        assert!(decode(&bytes[..bytes.len() - 1]).is_none());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode(&longer).is_none());
        assert!(decode(&[]).is_none());
    }
}
