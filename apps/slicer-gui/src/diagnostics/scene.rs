/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The scene in numbers, for `scene.txt` in a diagnostics file: how many
//! models, triangles and supports, what is hollowed, the printer and the
//! slice. No geometry and no names.

use std::fmt::Write as _;
use std::sync::Arc;

use sc_geom::df_supports::is_any_support_mesh;

use crate::scene_document::SceneDocument;

/// The text of `scene.txt` for the scene of `doc`.
pub(crate) fn report(doc: &SceneDocument) -> String {
    let mut out = String::new();
    let hollows = doc.hollows();
    let hollows = hollows.borrow();
    let counted = doc.with_scene(|spatial| {
        let (mut models, mut supports) = (Vec::new(), Vec::new());
        for id in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
            let Some(mesh) = spatial.graph().leaf_mesh_arc(id) else {
                continue;
            };
            let triangles = mesh.faces().len();
            if is_any_support_mesh(mesh) {
                supports.push(triangles);
            } else {
                let hollowed = hollows.iter().any(|h| Arc::ptr_eq(&h.hollow, mesh));
                models.push((triangles, hollowed));
            }
        }
        (models, supports)
    });
    let Some((models, supports)) = counted else {
        let _ = writeln!(out, "scene: none open");
        return out;
    };
    let origin = doc
        .with_source_path(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| format!(".{}", ext.to_ascii_lowercase()))
        })
        .flatten()
        .unwrap_or_else(|| "none".to_owned());
    let _ = writeln!(out, "scene: opened from a {origin} file");
    let _ = writeln!(out, "models: {}", models.len());
    for (i, (triangles, hollowed)) in models.iter().enumerate() {
        let hollowed = if *hollowed { ", hollowed" } else { "" };
        let _ = writeln!(out, "  model {}: {triangles} triangles{hollowed}", i + 1);
    }
    let _ = writeln!(
        out,
        "supports: {} set(s), {} triangles",
        supports.len(),
        supports.iter().sum::<usize>()
    );
    let selection = doc.print_selection();
    let named = |id: Option<&str>| id.map_or_else(|| "default".to_owned(), str::to_owned);
    let _ = writeln!(
        out,
        "printer: {}",
        named(selection.printer.as_ref().map(|p| p.as_str()))
    );
    let _ = writeln!(
        out,
        "resin: {}",
        named(selection.resin.as_ref().map(|r| r.as_str()))
    );
    if let Some(height) = selection.first_layer_height {
        let _ = writeln!(out, "first layer height: {height:?}");
    }
    if let Some(format) = &selection.output_format {
        let _ = writeln!(out, "printer file: {format:?}");
    }
    match doc.slice_layer_count() {
        Some(layers) => {
            let _ = writeln!(out, "slice: {layers} layers");
        }
        None => {
            let _ = writeln!(out, "slice: none");
        }
    }
    let _ = writeln!(
        out,
        "unsaved changes: {}",
        if doc.is_dirty() { "yes" } else { "no" }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_scene_fixtures::meshes_as_roots;

    #[test]
    fn no_scene_says_so() {
        let doc = SceneDocument::new();
        assert_eq!(report(&doc), "scene: none open\n");
    }

    #[test]
    fn a_scene_is_told_in_numbers_without_names() {
        let doc = SceneDocument::new();
        doc.install_graph(
            meshes_as_roots([unit_cube_mesh()]),
            r"C:\Users\Dana\Desktop\part.STL".into(),
        );
        let text = report(&doc);
        assert!(text.contains("scene: opened from a .stl file"), "{text}");
        assert!(!text.contains("part"), "{text}");
        assert!(!text.contains("Dana"), "{text}");
        assert!(text.contains("models: 1\n"), "{text}");
        assert!(text.contains("  model 1: 12 triangles\n"), "{text}");
        assert!(text.contains("supports: 0 set(s), 0 triangles"), "{text}");
        assert!(text.contains("printer: default"), "{text}");
        assert!(text.contains("slice: none"), "{text}");
    }
}
