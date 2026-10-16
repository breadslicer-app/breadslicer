/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::path::PathBuf;
use std::rc::Rc;

use sc_scene::NodeId;

use crate::scene_document::SceneDocument;
use crate::test_support::scenes::graph_with_one_object;

/// Returns a document holding [`graph_with_one_object`], and the node id of its
/// leaf, which the edits of a test target.
pub(crate) fn loaded_document() -> (Rc<SceneDocument>, NodeId) {
    let doc = SceneDocument::new();
    doc.install_graph(
        graph_with_one_object(),
        PathBuf::from("/tmp/loaded_document_fixture.obj"),
    );
    let node_id = doc
        .with_scene(|s| s.leaf_id_at_index(0))
        .flatten()
        .expect("fixture has one leaf");
    (doc, node_id)
}
