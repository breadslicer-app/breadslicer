/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The import notes of the open scene (#301): what a load changed or left
//! out of the file, said in the app and not only on the console.
//!
//! After a load the status bar says "Opened with N notes" while the scene is
//! open; a click on it opens the full list, one short line per note, with
//! the readers' own wording behind "Show details". A load without notes
//! shows nothing, and closing the scene or opening another file drops them.
//! All the notes of a load go in that one line, however many the file has.
//!
//! The notes name the file, or the object when the file names it, never an
//! internal node number. Lines a resin print never misses (OBJ comments,
//! texture coordinates, vertex normals, smoothing groups, an STL header
//! that is not text) stay on the console only.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use sc_mesh_formats::messages::{
    LoadMeshWarning, LoadObjWarning, LoadStlWarning, LoadThreeMfWarning, SceneLoadWarning,
};
use sc_mesh_spatial::SceneSpatial;
use sc_project_formats::{ProfileRefWarning, ReadProjectWarning};
use sc_scene::NodeId;
use slint::SharedString;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, InfoTexts};
use crate::i18n::ctr;
use crate::scene_loader::{
    PreparedScene, describe_obj_warning, describe_profile_ref_warning, describe_project_warning,
    describe_scene_warning_naming, describe_threemf_warning,
};

/// The most technical lines the details hold; an OBJ can repeat a skipped
/// line thousands of times.
const MAX_DETAIL_LINES: usize = 50;

/// The notes of one load: the short lines the list shows, and the readers'
/// technical lines behind "Show details".
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Notes {
    /// The file the notes are about, as the list names it.
    pub(crate) file: String,
    pub(crate) lines: Vec<SharedString>,
    pub(crate) details: Vec<String>,
}

impl Notes {
    /// The notes of the load `prepared`, read from `path`.
    pub(crate) fn of(path: &Path, prepared: &PreparedScene) -> Self {
        Self::from_warnings(
            path,
            &prepared.spatial,
            &prepared.warnings,
            &prepared.project_warnings,
            &prepared.profile_warnings,
        )
    }

    /// The notes of a load of `path` that read the scene `spatial` with
    /// these warnings.
    pub(crate) fn from_warnings(
        path: &Path,
        spatial: &SceneSpatial,
        warnings: &[LoadMeshWarning],
        project_warnings: &[ReadProjectWarning],
        profile_warnings: &[ProfileRefWarning],
    ) -> Self {
        let file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut notes = Self {
            file,
            ..Self::default()
        };
        notes.add_mesh_warnings(warnings, spatial);
        for warning in project_warnings {
            notes.add_project_warning(warning);
        }
        for warning in profile_warnings {
            notes.add(profile_note(warning), describe_profile_ref_warning(warning));
        }
        notes
    }

    fn add(&mut self, line: SharedString, detail: String) {
        self.lines.push(line);
        self.details.push(detail);
    }

    /// Adds 1 line for the technical lines `details`, said by `line` of
    /// their count; nothing when there are none.
    fn add_counted(&mut self, details: Vec<String>, line: impl Fn(usize) -> SharedString) {
        if details.is_empty() {
            return;
        }
        self.lines.push(line(details.len()));
        self.details.extend(details);
    }

    fn add_mesh_warnings(&mut self, warnings: &[LoadMeshWarning], spatial: &SceneSpatial) {
        let mut obj = ObjCounts::default();
        for warning in warnings {
            match warning {
                LoadMeshWarning::Obj(w) => obj.count(w),
                LoadMeshWarning::Stl(LoadStlWarning::HeaderNotText) => {}
                LoadMeshWarning::ThreeMf(w) => {
                    self.add(threemf_note(w), describe_threemf_warning(w));
                }
                LoadMeshWarning::Scene(w) => {
                    let name = |id: NodeId, object_name: Option<&str>| {
                        object_label(spatial, &self.file, id, object_name)
                    };
                    let line = scene_note(w, &name);
                    let detail = describe_scene_warning_naming(w, &name);
                    self.add(line, detail);
                }
            }
        }
        obj.add_to(self);
    }

    fn add_project_warning(&mut self, warning: &ReadProjectWarning) {
        let line = match warning {
            ReadProjectWarning::ThreeMf(w) => threemf_note(w),
            ReadProjectWarning::ProjectVersionUnsupported { version } => ctr!(
                "ImportNotes",
                "The project settings were saved by a newer version (format {}) and were ignored.",
                version
            ),
            ReadProjectWarning::UnrecognizedProfilePart { .. } => ctr!(
                "ImportNotes",
                "Ignored an unknown part among the profiles the file carries."
            ),
            ReadProjectWarning::ForeignProjectData { dialect } => ctr!(
                "ImportNotes",
                "The file carries project settings from {}; only the models were loaded.",
                crate::scene_loader::dialect_name(*dialect)
            ),
        };
        self.add(line, describe_project_warning(warning));
    }

    /// What the status bar says of these notes; empty when there are none.
    pub(crate) fn status_label(&self) -> SharedString {
        if self.lines.is_empty() {
            return SharedString::default();
        }
        ctr!(
            "Workspace",
            "Opened with {n} note",
            "Opened with {n} notes",
            self.lines.len()
        )
    }

    /// The message that lists the notes.
    pub(crate) fn texts(&self) -> InfoTexts {
        let mut message = ctr!("ImportNotes", "{} opened with these notes:", self.file).to_string();
        for line in &self.lines {
            message.push_str("\n• ");
            message.push_str(line);
        }
        let mut details: Vec<String> = self
            .details
            .iter()
            .take(MAX_DETAIL_LINES)
            .cloned()
            .collect();
        let more = self.details.len().saturating_sub(MAX_DETAIL_LINES);
        if more > 0 {
            details.push(format!("... and {more} more"));
        }
        InfoTexts {
            heading: ctr!("ImportNotes", "Import notes"),
            message: message.into(),
            details: details.join("\n").into(),
        }
    }
}

/// The OBJ lines a load skipped, counted by kind: an OBJ repeats them line by
/// line, the list says each kind once.
#[derive(Default)]
struct ObjCounts {
    no_vertices: bool,
    no_faces: bool,
    groups: Vec<String>,
    materials: Vec<String>,
    unknown: Vec<String>,
}

impl ObjCounts {
    fn count(&mut self, warning: &LoadObjWarning) {
        let detail = describe_obj_warning(warning);
        match warning {
            LoadObjWarning::NoVertices => self.no_vertices = true,
            LoadObjWarning::NoFaces => self.no_faces = true,
            LoadObjWarning::SkippedGroup { .. } => self.groups.push(detail),
            LoadObjWarning::SkippedMaterialReference { .. }
            | LoadObjWarning::SkippedMaterialLibrary { .. } => self.materials.push(detail),
            LoadObjWarning::SkippedUnknownDirective { .. } => self.unknown.push(detail),
            // Data a resin print never uses.
            LoadObjWarning::SkippedTextureCoordinates { .. }
            | LoadObjWarning::SkippedVertexNormal { .. }
            | LoadObjWarning::SkippedSmoothingGroup { .. }
            | LoadObjWarning::SkippedComment { .. } => {}
        }
    }

    fn add_to(self, notes: &mut Notes) {
        if self.no_vertices {
            notes.add(
                ctr!("ImportNotes", "The OBJ file has no vertices."),
                describe_obj_warning(&LoadObjWarning::NoVertices),
            );
        }
        if self.no_faces {
            notes.add(
                ctr!("ImportNotes", "The OBJ file has no faces."),
                describe_obj_warning(&LoadObjWarning::NoFaces),
            );
        }
        notes.add_counted(self.groups, |n| {
            ctr!(
                "ImportNotes",
                "Ignored {n} OBJ group line; its faces were loaded with their object.",
                "Ignored {n} OBJ group lines; their faces were loaded with their objects.",
                n
            )
        });
        notes.add_counted(self.materials, |n| {
            ctr!(
                "ImportNotes",
                "Ignored {n} OBJ material line: materials are not supported yet.",
                "Ignored {n} OBJ material lines: materials are not supported yet.",
                n
            )
        });
        notes.add_counted(self.unknown, |n| {
            ctr!(
                "ImportNotes",
                "Skipped {n} OBJ line this reader does not handle, such as lines or points.",
                "Skipped {n} OBJ lines this reader does not handle, such as lines or points.",
                n
            )
        });
    }
}

/// How a note names the object `id`: its own name when the file gives one,
/// the file's name when it holds that one object, else the object list's
/// name for it.
fn object_label(
    spatial: &SceneSpatial,
    file: &str,
    id: NodeId,
    object_name: Option<&str>,
) -> String {
    if let Some(name) = object_name {
        return format!("'{name}'");
    }
    let graph = spatial.graph();
    if graph.reachable_leaf_count() <= 1 {
        return file.to_string();
    }
    spatial
        .leaf_index_of(id)
        .and_then(|row| crate::viewport::object_names(graph).into_iter().nth(row))
        .map(|name| format!("{file}, {name}"))
        .unwrap_or_else(|| file.to_string())
}

fn scene_note(
    warning: &SceneLoadWarning,
    name: &dyn Fn(NodeId, Option<&str>) -> String,
) -> SharedString {
    match warning {
        SceneLoadWarning::SharedVertices { leaves } => {
            let labels: Vec<String> = leaves
                .iter()
                .map(|leaf| name(leaf.node_id, leaf.object_name.as_deref()))
                .collect();
            ctr!(
                "ImportNotes",
                "These objects share vertices in the file: {}.",
                labels.join(", ")
            )
        }
        SceneLoadWarning::DuplicateFacesDropped { leaves } => {
            let total: usize = leaves.iter().map(|leaf| leaf.faces).sum();
            let labels = match leaves.as_slice() {
                [leaf] => name(leaf.node_id, leaf.object_name.as_deref()),
                _ => leaves
                    .iter()
                    .map(|leaf| {
                        format!(
                            "{} ({})",
                            name(leaf.node_id, leaf.object_name.as_deref()),
                            leaf.faces
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            };
            ctr!(
                "ImportNotes",
                "Removed {n} duplicate face from {}: an exact copy of another face, so the shape did not change.",
                "Removed {n} duplicate faces from {}: exact copies of other faces, so the shape did not change.",
                total,
                labels
            )
        }
    }
}

fn threemf_note(warning: &LoadThreeMfWarning) -> SharedString {
    match warning {
        LoadThreeMfWarning::MaterialsDropped { group_count } => ctr!(
            "ImportNotes",
            "Ignored {n} material group: materials are not supported yet.",
            "Ignored {n} material groups: materials are not supported yet.",
            *group_count
        ),
        LoadThreeMfWarning::UnbuiltObjectSkipped { id, name } => {
            let label = match name {
                Some(name) => format!("'{name}'"),
                None => id.to_string(),
            };
            ctr!(
                "ImportNotes",
                "Skipped object {}: the file does not place it on the plate.",
                label
            )
        }
        LoadThreeMfWarning::NonModelObjectTypeFlattened { id, object_type } => ctr!(
            "ImportNotes",
            "Object {} is marked as '{}' in the file; it was loaded as a plain model.",
            id,
            object_type.as_str()
        ),
        LoadThreeMfWarning::SliceDataIgnored { stack_count } => ctr!(
            "ImportNotes",
            "Ignored the slices stored in the file ({n} stack); the full models were loaded.",
            "Ignored the slices stored in the file ({n} stacks); the full models were loaded.",
            *stack_count
        ),
        LoadThreeMfWarning::EmptyBuild => {
            ctr!("ImportNotes", "The file places no objects on the plate.")
        }
        LoadThreeMfWarning::DeclaredFacetNormalsMismatch { object_id } => ctr!(
            "ImportNotes",
            "Dropped the face normals stored for object {}: its mesh changed after they were saved.",
            object_id
        ),
        LoadThreeMfWarning::DeclaredFacetNormalsMalformed { .. } => ctr!(
            "ImportNotes",
            "Ignored a damaged part with stored face normals."
        ),
        LoadThreeMfWarning::UntypedPartsSkipped { parts } => ctr!(
            "ImportNotes",
            "Skipped {n} file part with no content type, such as another slicer's settings or pictures.",
            "Skipped {n} file parts with no content type, such as another slicer's settings or pictures.",
            parts.len()
        ),
    }
}

fn profile_note(warning: &ProfileRefWarning) -> SharedString {
    match warning {
        ProfileRefWarning::PrinterMissing { id } => ctr!(
            "ImportNotes",
            "The project uses the printer profile \"{}\", which is not installed.",
            id
        ),
        ProfileRefWarning::PrinterEmbeddedOnly { id } => ctr!(
            "ImportNotes",
            "The project uses the printer profile \"{}\", which is not installed; the file carries a copy of it.",
            id
        ),
        ProfileRefWarning::ResinMissing { id } => ctr!(
            "ImportNotes",
            "The project uses the resin profile \"{}\", which is not installed.",
            id
        ),
        ProfileRefWarning::ResinEmbeddedOnly { id } => ctr!(
            "ImportNotes",
            "The project uses the resin profile \"{}\", which is not installed; the file carries a copy of it.",
            id
        ),
    }
}

/// The notes of the open scene, behind the status bar's line.
pub(crate) struct ImportNotes {
    ui: slint::Weak<MainWindow>,
    dialogs: Rc<DialogManager>,
    notes: RefCell<Notes>,
}

impl ImportNotes {
    pub(crate) fn new(ui: slint::Weak<MainWindow>, dialogs: Rc<DialogManager>) -> Self {
        Self {
            ui,
            dialogs,
            notes: RefCell::new(Notes::default()),
        }
    }

    /// Takes `notes` as the open scene's, in place of any before.
    pub(crate) fn set(&self, notes: Notes) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_import_notes(notes.status_label());
        }
        *self.notes.borrow_mut() = notes;
    }

    /// Drops the notes, as the scene closed.
    pub(crate) fn clear(&self) {
        self.set(Notes::default());
    }

    /// Opens the list of the notes; nothing when there are none.
    pub(crate) fn open(&self) {
        let notes = self.notes.borrow().clone();
        if notes.lines.is_empty() {
            return;
        }
        self.dialogs.show_info(move || notes.texts());
    }
}

#[cfg(test)]
mod import_notes_tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use sc_mesh_formats::messages::DuplicateFacesLeaf;
    use sc_scene_fixtures::trio_as_roots;

    use super::*;
    use crate::build_volume::BuildVolume;
    use crate::mesh_loader::start_load_with_ticket;
    use crate::mesh_render::{DeviceLoss, DeviceToken};
    use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
    use crate::scene_loader::SceneLoader;
    use crate::test_support::gpu_devices::shared_test_device;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    fn obj(warning: LoadObjWarning) -> LoadMeshWarning {
        LoadMeshWarning::Obj(warning)
    }

    #[test]
    fn the_obj_lines_go_in_one_note_per_kind_and_the_harmless_ones_in_none() {
        let spatial = SceneSpatial::from_graph(trio_as_roots());
        let mut warnings: Vec<LoadMeshWarning> = (1..=500)
            .flat_map(|line| {
                [
                    obj(LoadObjWarning::SkippedComment { line }),
                    obj(LoadObjWarning::SkippedVertexNormal { line }),
                    obj(LoadObjWarning::SkippedTextureCoordinates { line }),
                    obj(LoadObjWarning::SkippedSmoothingGroup { line }),
                ]
            })
            .collect();
        warnings.extend((1..=3).map(|line| {
            obj(LoadObjWarning::SkippedGroup {
                line,
                name: format!("g{line}"),
            })
        }));
        warnings.push(LoadMeshWarning::Stl(LoadStlWarning::HeaderNotText));
        let notes =
            Notes::from_warnings(Path::new("C:/m/model.obj"), &spatial, &warnings, &[], &[]);
        assert_eq!(
            notes.lines,
            vec![SharedString::from(
                "Ignored 3 OBJ group lines; their faces were loaded with their objects."
            )]
        );
        assert_eq!(notes.details.len(), 3, "{:?}", notes.details);
        assert_eq!(notes.status_label(), "Opened with 1 note");
    }

    #[test]
    fn a_harmless_load_has_no_notes_and_says_nothing() {
        let spatial = SceneSpatial::from_graph(trio_as_roots());
        let warnings = [obj(LoadObjWarning::SkippedComment { line: 1 })];
        let notes = Notes::from_warnings(Path::new("a.obj"), &spatial, &warnings, &[], &[]);
        assert!(notes.lines.is_empty());
        assert_eq!(notes.status_label(), "");
    }

    #[test]
    fn duplicate_faces_name_the_object_by_the_file_and_the_object_list() {
        let spatial = SceneSpatial::from_graph(trio_as_roots());
        let second = spatial.leaf_id_at_index(1).expect("the trio has 3 leaves");
        let third = spatial.leaf_id_at_index(2).expect("the trio has 3 leaves");
        let warnings = [LoadMeshWarning::Scene(
            SceneLoadWarning::DuplicateFacesDropped {
                leaves: vec![
                    DuplicateFacesLeaf {
                        node_id: second,
                        object_name: None,
                        faces: 2160,
                    },
                    DuplicateFacesLeaf {
                        node_id: third,
                        object_name: Some("Werewolf".to_string()),
                        faces: 12,
                    },
                ],
            },
        )];
        let notes = Notes::from_warnings(Path::new("C:/m/trio.3mf"), &spatial, &warnings, &[], &[]);
        assert_eq!(
            notes.lines,
            vec![SharedString::from(
                "Removed 2172 duplicate faces from trio.3mf, Object 2 (2160); 'Werewolf' (12): \
                 exact copies of other faces, so the shape did not change."
            )]
        );
        assert_eq!(
            notes.details,
            vec![
                "removed 2172 duplicate face(s), exact copies of other faces: trio.3mf, Object 2 \
                 (2160), 'Werewolf' (12)"
                    .to_string()
            ]
        );
    }

    #[test]
    fn the_list_has_every_note_and_the_readers_lines_behind_details() {
        let spatial = SceneSpatial::from_graph(trio_as_roots());
        let warnings = [LoadMeshWarning::ThreeMf(
            LoadThreeMfWarning::UntypedPartsSkipped {
                parts: vec!["/Metadata/a.config".to_string(), "/b.webp".to_string()],
            },
        )];
        let project = [ReadProjectWarning::ForeignProjectData {
            dialect: sc_project_formats::ForeignDialect::Cura,
        }];
        let notes =
            Notes::from_warnings(Path::new("plate.3mf"), &spatial, &warnings, &project, &[]);
        let texts = notes.texts();
        assert_eq!(texts.heading, "Import notes");
        assert_eq!(
            texts.message,
            "plate.3mf opened with these notes:\n\
             • Skipped 2 file parts with no content type, such as another slicer's settings or \
             pictures.\n\
             • The file carries project settings from Cura; only the models were loaded."
        );
        assert_eq!(
            texts.details,
            "skipped 2 package part(s) with no content type: /Metadata/a.config, /b.webp\n\
             the file carries project data written by Cura; only the model data was loaded"
        );
        assert_eq!(notes.status_label(), "Opened with 2 notes");
    }

    // --- a whole load, on the GPU ------------------------------------

    /// The face the STL below stores twice.
    const DUPLICATED_FACET: &str = "facet normal 0 0 -1
 outer loop
  vertex 0 0 0
  vertex 0 10 0
  vertex 10 0 0
 endloop
endfacet
";

    /// An ASCII STL of a tetrahedron that stores its first face twice, and
    /// gives it no name.
    const TETRAHEDRON_WITH_A_DUPLICATE: &str = "solid
facet normal 0 0 -1
 outer loop
  vertex 0 0 0
  vertex 0 10 0
  vertex 10 0 0
 endloop
endfacet
facet normal 0 0 -1
 outer loop
  vertex 0 0 0
  vertex 0 10 0
  vertex 10 0 0
 endloop
endfacet
facet normal 0 -1 0
 outer loop
  vertex 0 0 0
  vertex 10 0 0
  vertex 0 0 10
 endloop
endfacet
facet normal -1 0 0
 outer loop
  vertex 0 0 0
  vertex 0 0 10
  vertex 0 10 0
 endloop
endfacet
facet normal 1 1 1
 outer loop
  vertex 10 0 0
  vertex 0 10 0
  vertex 0 0 10
 endloop
endfacet
endsolid
";

    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>"#;

    const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

    const TETRAHEDRON_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources>
  <object id="1" type="model">
   <mesh>
    <vertices>
     <vertex x="0" y="0" z="0"/>
     <vertex x="10" y="0" z="0"/>
     <vertex x="0" y="10" z="0"/>
     <vertex x="0" y="0" z="10"/>
    </vertices>
    <triangles>
     <triangle v1="0" v2="2" v3="1"/>
     <triangle v1="0" v2="1" v3="3"/>
     <triangle v1="0" v2="3" v3="2"/>
     <triangle v1="1" v2="2" v3="3"/>
    </triangles>
   </mesh>
  </object>
 </resources>
 <build>
  <item objectid="1"/>
 </build>
</model>"#;

    /// Writes a 3MF holding a tetrahedron and a part the content-type map
    /// does not cover, as FDM slicers store their settings.
    fn write_threemf_with_an_untyped_part(path: &Path) {
        let file = std::fs::File::create(path).expect("the file is created");
        let mut archive = zip::write::ZipWriter::new(file);
        let items: [(&str, &[u8]); 4] = [
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
            ("_rels/.rels", ROOT_RELS.as_bytes()),
            ("3D/3dmodel.model", TETRAHEDRON_MODEL.as_bytes()),
            ("Metadata/slicer_settings.config", b"{}"),
        ];
        for (name, bytes) in items {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .expect("the entry starts");
            archive.write_all(bytes).expect("the entry writes");
        }
        archive.finish().expect("the archive finishes");
    }

    /// What a window shows after opening `path`: the status bar's line, the
    /// text of the list a click on it opens, and the line once the scene
    /// closes. `None` when no adapter is available.
    fn open_in_a_window(path: PathBuf) -> Option<(String, Option<String>, String)> {
        let (device, queue) = shared_test_device()?;
        Some(on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("the headless platform creates a window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialogs = DialogManager::new(&bus);
            dialogs.attach(&window);
            let loader = SceneLoader::new(&window, dialogs, DeviceLoss::default());
            loader.set_gpu(device, DeviceToken::next(), queue);
            let ticket = loader.begin_load().expect("no load is in flight");
            start_load_with_ticket(ticket, path, &loader, BuildVolume::new(100.0, 60.0, 80.0));
            let deadline = Instant::now() + Duration::from_secs(60);
            while loader.take_finished_load().is_none() {
                assert!(Instant::now() < deadline, "the load ends within 60 s");
                std::thread::sleep(Duration::from_millis(10));
            }
            let line = window.get_import_notes().to_string();
            loader.open_import_notes();
            let list = window
                .get_message_open()
                .then(|| window.get_message().text.to_string());
            loader.invalidate_and_clear();
            (line, list, window.get_import_notes().to_string())
        }))
    }

    #[test]
    fn opening_an_stl_with_duplicate_faces_says_so_naming_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("werewolf.stl");
        std::fs::write(&path, TETRAHEDRON_WITH_A_DUPLICATE).expect("the STL writes");
        let Some((line, list, closed)) = open_in_a_window(path) else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        assert_eq!(line, "Opened with 1 note");
        assert_eq!(
            list.as_deref(),
            Some(
                "werewolf.stl opened with these notes:\n\
                 • Removed 1 duplicate face from werewolf.stl: an exact copy of another face, so \
                 the shape did not change."
            )
        );
        assert_eq!(closed, "", "closing the scene drops the notes");
    }

    #[test]
    fn opening_a_threemf_with_untyped_parts_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plate.3mf");
        write_threemf_with_an_untyped_part(&path);
        let Some((line, list, _)) = open_in_a_window(path) else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        assert_eq!(line, "Opened with 1 note");
        let list = list.expect("a click opens the list");
        assert!(
            list.contains(
                "• Skipped 1 file part with no content type, such as another slicer's settings \
                 or pictures."
            ),
            "{list}"
        );
    }

    #[test]
    fn opening_a_clean_file_shows_nothing_new() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("clean.stl");
        let clean = TETRAHEDRON_WITH_A_DUPLICATE.replacen(DUPLICATED_FACET, "", 1);
        std::fs::write(&path, clean).expect("the STL writes");
        let Some((line, list, _)) = open_in_a_window(path) else {
            eprintln!("skipping: no wgpu adapter available in this environment");
            return;
        };
        assert_eq!(line, "");
        assert_eq!(list, None, "nothing to open");
    }
}
