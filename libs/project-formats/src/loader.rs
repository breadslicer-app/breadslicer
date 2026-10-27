/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Loading a scene from a file, whatever its format.
//!
//! - [`load_scene`] reads a mesh or project file into a [`LoadedScene`]; it
//!   owns the extension dispatch, the size cap, the byte progress and the
//!   shared-vertex post-check.
//! - [`SUPPORTED_SCENE_FILE_EXTENSIONS`] is the list the dispatch accepts; file
//!   dialogs and drop handlers filter on it.

use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sc_bread_format::SupportDocument;
use sc_mesh_formats::LoadedMesh;
use sc_mesh_formats::error::LoadMeshError;
use sc_mesh_formats::filetypes::infer_mesh_file_format_from_extension;
use sc_mesh_formats::loader::{load_mesh_from_reader, scene_shared_vertices_warning};
use sc_mesh_formats::messages::LoadMeshWarning;
use sc_scene::Scene;
use sc_sl1cer::contour::ContourStack;

use crate::error::ReadProjectError;
use crate::filetypes::{ProjectFileType, infer_project_file_type_from_extension};
use crate::messages::ReadProjectWarning;
use crate::payload::ProjectPayload;
use crate::progress_reader::ProgressReader;
use crate::read::{LoadedProject, read_project_with_progress};

pub const SUPPORTED_SCENE_FILE_EXTENSIONS: &[&str] =
    &["bread", "3mf", "obj", "stl", "stla", "stlb"];

/// Returns whether `path`'s extension is in
/// [`SUPPORTED_SCENE_FILE_EXTENSIONS`], matching case-insensitively.
pub fn is_supported_scene_file_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    SUPPORTED_SCENE_FILE_EXTENSIONS.iter().any(|&e| e == ext)
}

/// A byte-progress report from [`load_scene`]. The unit depends on the route:
/// on-disk bytes for a mesh file, decompressed bytes for a project container;
/// either way `done_bytes / total_bytes` is the fraction to display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// Everything loading a scene file produces.
#[derive(Debug)]
pub struct LoadedScene {
    pub content: LoadedContent,
    pub mesh_warnings: Vec<LoadMeshWarning>,
    pub project_warnings: Vec<ReadProjectWarning>,
}

/// The scene of a loaded file, by whether the file carries project data that
/// this build reads.
// Built and moved once per load; boxing the project would only cost every
// caller a deref in its match.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum LoadedContent {
    /// A file without such data: a mesh file, a 3MF without the project part,
    /// or a 3MF whose project part has a version that this build cannot read.
    /// Its transforms are in no known printer frame.
    Mesh(Scene),
    /// A project. Its transforms are in the frame of the printer that it was
    /// saved for.
    Project(ProjectContent),
}

/// The scene of a project file and the project data that it carries.
#[derive(Debug)]
pub struct ProjectContent {
    pub scene: Scene,
    pub payload: ProjectPayload,
    /// `None` unless the file stores both the slices and their token.
    pub persisted_slices: Option<PersistedSlices>,
    /// Supports stored as data, whose leaves arrive with an empty mesh for the
    /// application to rebuild (see [`crate::LoadedProject::supports`]).
    pub supports: Vec<SupportDocument>,
}

/// Slices that a project file stores, with the token that they were saved with.
#[derive(Debug)]
pub struct PersistedSlices {
    /// The slices, mapped back to a contour stack. They arrive unvalidated:
    /// revalidate them against `fingerprint` before trusting them.
    pub contours: ContourStack,
    /// The opaque validity token of `contours`, as the file holds it.
    pub fingerprint: String,
}

/// Why [`load_scene`] produced no scene.
#[derive(Debug)]
pub enum LoadSceneError {
    /// The path's extension is not in [`SUPPORTED_SCENE_FILE_EXTENSIONS`];
    /// nothing was opened.
    UnsupportedExtension { path: PathBuf },
    /// The file exceeds the caller's size cap; nothing was parsed.
    FileTooLarge { actual_bytes: u64, limit_bytes: u64 },
    /// The file could not be opened or measured.
    Io(std::io::Error),
    /// The mesh route failed to parse the file.
    Mesh(LoadMeshError),
    /// The project route failed to parse the file.
    Project(ReadProjectError),
}

impl std::fmt::Display for LoadSceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedExtension { path } => write!(
                f,
                "cannot open {}: the extension names no supported scene format",
                path.display(),
            ),
            Self::FileTooLarge {
                actual_bytes,
                limit_bytes,
            } => write!(
                f,
                "file is {actual_bytes} bytes, which exceeds the {limit_bytes} byte limit",
            ),
            Self::Io(e) => write!(f, "I/O error while opening the scene file: {e}"),
            Self::Mesh(e) => write!(f, "{e}"),
            Self::Project(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadSceneError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Mesh(e) => Some(e),
            Self::Project(e) => Some(e),
            Self::UnsupportedExtension { .. } | Self::FileTooLarge { .. } => None,
        }
    }
}

/// Loads the scene from the file at `path`. Project-capable extensions go
/// through the project reader, everything else in
/// [`SUPPORTED_SCENE_FILE_EXTENSIONS`] through the mesh loader.
///
/// `max_file_size` is the upper bound on the on-disk byte count; `None`
/// disables the check.
///
/// On success the last `on_progress` call reports `done_bytes == total_bytes`.
pub fn load_scene(
    path: &Path,
    max_file_size: Option<u64>,
    mut on_progress: impl FnMut(LoadProgress),
) -> Result<LoadedScene, LoadSceneError> {
    if !is_supported_scene_file_extension(path) {
        return Err(LoadSceneError::UnsupportedExtension {
            path: path.to_path_buf(),
        });
    }
    if let Some(cap) = max_file_size
        && let Ok(meta) = std::fs::metadata(path)
        && meta.len() > cap
    {
        return Err(LoadSceneError::FileTooLarge {
            actual_bytes: meta.len(),
            limit_bytes: cap,
        });
    }
    match infer_project_file_type_from_extension(path) {
        Ok(file_type) => open_project(path, file_type, max_file_size, &mut on_progress),
        Err(_) => open_mesh(path, max_file_size, &mut on_progress),
    }
}

fn open_mesh<F: FnMut(LoadProgress)>(
    path: &Path,
    max_file_size: Option<u64>,
    on_progress: &mut F,
) -> Result<LoadedScene, LoadSceneError> {
    let kind = infer_mesh_file_format_from_extension(path)
        .map_err(LoadMeshError::from)
        .map_err(LoadSceneError::Mesh)?;
    let file = File::open(path).map_err(LoadSceneError::Io)?;
    let mut tick = |position: u64, total: u64| {
        on_progress(LoadProgress {
            done_bytes: position,
            total_bytes: total,
        })
    };
    let reader = ProgressReader::new(file, &mut tick).map_err(LoadSceneError::Io)?;
    // The measured total also covers the caller's cap when the metadata
    // precheck in `load_scene` could not run.
    let total = reader.total_bytes();
    if let Some(cap) = max_file_size
        && total > cap
    {
        return Err(LoadSceneError::FileTooLarge {
            actual_bytes: total,
            limit_bytes: cap,
        });
    }
    let mut buffered = BufReader::new(reader);
    let LoadedMesh { graph, warnings } =
        load_mesh_from_reader(kind, &mut buffered).map_err(LoadSceneError::Mesh)?;
    drop(buffered);
    tick(total, total);
    Ok(LoadedScene {
        content: LoadedContent::Mesh(graph),
        mesh_warnings: warnings,
        project_warnings: Vec::new(),
    })
}

fn open_project<F: FnMut(LoadProgress)>(
    path: &Path,
    file_type: ProjectFileType,
    max_file_size: Option<u64>,
    on_progress: &mut F,
) -> Result<LoadedScene, LoadSceneError> {
    let mut file = File::open(path).map_err(LoadSceneError::Io)?;
    // Same duty as the mesh route's measured total: the cap must hold even when
    // the metadata precheck could not run.
    let total = file.seek(SeekFrom::End(0)).map_err(LoadSceneError::Io)?;
    file.seek(SeekFrom::Start(0)).map_err(LoadSceneError::Io)?;
    if let Some(cap) = max_file_size
        && total > cap
    {
        return Err(LoadSceneError::FileTooLarge {
            actual_bytes: total,
            limit_bytes: cap,
        });
    }
    let mut reader = BufReader::new(file);
    let mut announced_total: u64 = 0;
    let LoadedProject {
        scene,
        warnings: project_warnings,
        payload,
        slices,
        slice_fingerprint,
        supports,
    } = read_project_with_progress(&mut reader, file_type, |p| {
        announced_total = p.total_bytes;
        on_progress(LoadProgress {
            done_bytes: p.decompressed_bytes,
            total_bytes: p.total_bytes,
        });
    })
    .map_err(LoadSceneError::Project)?;
    // The plain loader's post-read check; the project reader sits below that
    // dispatch level, so it runs here.
    let mut mesh_warnings = Vec::new();
    if let Some(warning) = scene_shared_vertices_warning(&scene) {
        mesh_warnings.push(LoadMeshWarning::Scene(warning));
    }
    on_progress(LoadProgress {
        done_bytes: announced_total,
        total_bytes: announced_total,
    });
    let content = match payload {
        Some(payload) => LoadedContent::Project(ProjectContent {
            scene,
            payload,
            persisted_slices: slices
                .zip(slice_fingerprint)
                .map(|(contours, fingerprint)| PersistedSlices {
                    contours,
                    fingerprint,
                }),
            supports,
        }),
        None => LoadedContent::Mesh(scene),
    };
    Ok(LoadedScene {
        content,
        mesh_warnings,
        project_warnings,
    })
}
