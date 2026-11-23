/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The creator's credit a `.bread` file carries in its META (#745).
//!
//! A file may say who made it (`designer`), under what licence
//! (`license`), where it came from (`source`) and what it is called
//! (`title`), for the whole file or for one node. The view shows the
//! credit of the model picked, or of the file: what it carries, nothing
//! more. A source opens in the browser only when it is an `http` or
//! `https` address; anything else shows as plain text.

use sc_project_formats::FileMetadata;
use sc_scene::{NodeId, Scene};

/// What a file says of who made a model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Credit {
    pub(crate) title: Option<String>,
    pub(crate) designer: Option<String>,
    pub(crate) license: Option<String>,
    pub(crate) source: Option<String>,
}

impl Credit {
    /// The credit `metadata` states for `node`: its own keys, or those of
    /// the nearest node above it that states any, or the file's. `None`
    /// when nothing there names a creator, a licence or a source.
    pub(crate) fn of(
        metadata: &[FileMetadata],
        graph: Option<&Scene>,
        node: Option<NodeId>,
    ) -> Option<Self> {
        let mut at = node;
        loop {
            if let Some(credit) = Self::stated(metadata, at) {
                return Some(credit);
            }
            match at {
                None => return None,
                Some(id) => at = graph.and_then(|g| g.parent_of(id)),
            }
        }
    }

    /// The keys `metadata` states for exactly `node` (`None`: the file),
    /// when they credit someone.
    fn stated(metadata: &[FileMetadata], node: Option<NodeId>) -> Option<Self> {
        let text = |key: &str| {
            metadata
                .iter()
                .find(|m| m.node == node && m.key == key)
                .and_then(|m| String::from_utf8(m.value.clone()).ok())
                .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|value| !value.is_empty())
        };
        let credit = Self {
            title: text("title"),
            designer: text("designer"),
            license: text("license"),
            source: text("source"),
        };
        (credit.designer.is_some() || credit.license.is_some() || credit.source.is_some())
            .then_some(credit)
    }

    /// The line the view shows: the title in quotes, who designed it and
    /// the licence, those it has.
    pub(crate) fn line(&self, designed_by: impl Fn(&str) -> String) -> String {
        let mut parts = Vec::new();
        if let Some(title) = &self.title {
            parts.push(format!("“{title}”"));
        }
        if let Some(designer) = &self.designer {
            parts.push(designed_by(designer));
        }
        if let Some(license) = &self.license {
            parts.push(license.clone());
        }
        if self.link().is_none()
            && let Some(source) = &self.source
        {
            parts.push(source.clone());
        }
        parts.join("  ·  ")
    }

    /// The source, when it is a web address the browser may open.
    pub(crate) fn link(&self) -> Option<&str> {
        let source = self.source.as_deref()?;
        let lower = source.to_ascii_lowercase();
        let web = (lower.starts_with("https://") || lower.starts_with("http://"))
            && source.len() > "https://".len()
            && !source.chars().any(char::is_whitespace);
        web.then_some(source)
    }
}

#[cfg(test)]
mod credit_tests {
    use super::*;

    fn meta(node: Option<NodeId>, key: &str, value: &str) -> FileMetadata {
        FileMetadata {
            node,
            key: key.to_owned(),
            value: value.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_file_s_credit_reads_its_keys_and_its_link() {
        let metadata = [
            meta(None, "title", "Goblin  Archer"),
            meta(None, "designer", "Fulano"),
            meta(None, "license", "CC-BY-4.0"),
            meta(None, "source", "https://example.com/goblin"),
        ];
        let credit = Credit::of(&metadata, None, None).expect("a credit");
        assert_eq!(
            credit.line(|d| format!("by {d}")),
            "“Goblin Archer”  ·  by Fulano  ·  CC-BY-4.0"
        );
        assert_eq!(credit.link(), Some("https://example.com/goblin"));
    }

    #[test]
    fn a_source_that_is_no_web_address_shows_as_text_and_opens_nothing() {
        for source in [
            "file:///C:/x.exe",
            "javascript:alert(1)",
            "C:\\run.bat",
            "https://",
        ] {
            let metadata = [meta(None, "source", source)];
            let credit = Credit::of(&metadata, None, None).expect("a credit");
            assert_eq!(credit.link(), None, "{source}");
            assert!(credit.line(|d| d.to_owned()).contains(source), "{source}");
        }
    }

    #[test]
    fn a_title_alone_credits_no_one() {
        let metadata = [meta(None, "title", "Goblin")];
        assert_eq!(Credit::of(&metadata, None, None), None);
        assert_eq!(Credit::of(&[], None, None), None);
    }

    #[test]
    fn a_node_s_own_credit_wins_over_the_file_s() {
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                sc_mesh_core::transform::Transform3D::Identity,
                sc_scene::node::LeafPayload::Mesh(std::sync::Arc::new(
                    sc_mesh_core::IndexedMesh::new(None, Vec::new(), Vec::new()),
                )),
            )
            .unwrap();
        graph.add_root(leaf).unwrap();
        let metadata = [
            meta(None, "designer", "Studio"),
            meta(Some(leaf), "designer", "Fulano"),
        ];
        let own = Credit::of(&metadata, Some(&graph), Some(leaf)).unwrap();
        assert_eq!(own.designer.as_deref(), Some("Fulano"));
        let file = Credit::of(&metadata[..1], Some(&graph), Some(leaf)).unwrap();
        assert_eq!(file.designer.as_deref(), Some("Studio"));
    }
}
