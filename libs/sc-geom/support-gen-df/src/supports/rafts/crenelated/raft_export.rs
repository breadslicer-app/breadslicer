//! The raft step of DragonFruit `features/export/logic/ExportManager.ts`
//! (`// 4. Add Raft`): one raft per model, from that model's support roots,
//! kept clear of every visible model standing on the plate.

use std::collections::{HashMap, HashSet};

use super::geometry::build_line_raft_edge_pairs::{
    BuildLineRaftEdgePairsOptions, build_line_raft_edge_pairs,
};
use super::geometry::compute_raft_footprint::{inflate_model_plate_clearance, raft_band_top_mm};
use super::geometry::generate_chamfered_beam::generate_chamfered_beam;
use super::geometry::generate_raft_from_footprint::build_raft_footprint_meshes;
use super::geometry::generate_unioned_line_raft_mesh::{
    filter_line_raft_edges, generate_unioned_line_raft_mesh,
};
use super::geometry::model_plate_footprint::{PlateFootprintSource, collect_model_plate_footprint};
use super::raft_types::{RaftBottomMode, RaftSettings, SupportBaseCircle};
use crate::clipper::ClipperError;
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::math_utils::js_max;
use crate::three::{Vec3, Vector2};

/// A support root as the raft reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct RaftRoot {
    /// `root.modelId` (`""` reads as no model, as the JavaScript's falsy test).
    pub model_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub diameter: f64,
}

/// One `THREE.Group` named `Raft`: its meshes in the order they were added.
#[derive(Debug, Clone)]
pub struct RaftGroup {
    /// The model key (`__orphan__` for roots without a model).
    pub model_key: String,
    pub meshes: Vec<BufferGeometry>,
}

/// The raft settings store: the global settings and per-model overrides.
#[derive(Debug, Clone, Default)]
pub struct RaftSettingsStore {
    pub global: RaftSettings,
    pub per_model: HashMap<String, RaftSettings>,
}

impl RaftSettingsStore {
    /// `getRaftSettingsForModel(modelId)`.
    pub fn for_model(&self, model_id: Option<&str>) -> RaftSettings {
        match model_id {
            Some(id) if !id.is_empty() => self.per_model.get(id).copied().unwrap_or(self.global),
            _ => self.global,
        }
    }
}

/// The rafts `ExportManager` adds when `includeRaft` is set. `roots` are in
/// support-state order; `scoped_model_ids` is the export's model filter;
/// `models` are the scene's models (only those with `visible: true` are
/// cleared).
pub fn build_export_rafts(
    roots: &[RaftRoot],
    scoped_model_ids: Option<&HashSet<String>>,
    settings: &RaftSettingsStore,
    models: &[PlateFootprintSource],
) -> Result<Vec<RaftGroup>, ClipperError> {
    if settings.global.bottom_mode == RaftBottomMode::Off {
        return Ok(Vec::new());
    }
    // A Map keyed by model: groups in order of their first root.
    let mut order: Vec<String> = Vec::new();
    let mut by_model: HashMap<String, Vec<&RaftRoot>> = HashMap::new();
    for root in roots {
        if let Some(scope) = scoped_model_ids {
            match root.model_id.as_deref() {
                Some(id) if !id.is_empty() && scope.contains(id) => {}
                _ => continue,
            }
        }
        let mid = root
            .model_id
            .clone()
            .unwrap_or_else(|| "__orphan__".to_owned());
        by_model
            .entry(mid.clone())
            .or_insert_with(|| {
                order.push(mid);
                Vec::new()
            })
            .push(root);
    }
    let clearance_targets: Vec<PlateFootprintSource> = models
        .iter()
        // `models.filter((model) => model.visible)`: an unset flag is out.
        .filter(|m| m.visible == Some(true))
        .cloned()
        .collect();

    let mut groups = Vec::new();
    for model_key in order {
        let roots = &by_model[&model_key];
        if roots.is_empty() {
            continue;
        }
        let model_id = (model_key != "__orphan__").then_some(model_key.as_str());
        let raft = settings.for_model(model_id);
        let circles: Vec<SupportBaseCircle> = roots
            .iter()
            .map(|r| SupportBaseCircle {
                x: r.x,
                y: r.y,
                r: r.diameter / 2.0,
            })
            .collect();
        let clearance = collect_model_plate_footprint(&clearance_targets, raft_band_top_mm(&raft))?;
        let parts = build_raft_footprint_meshes(&circles, &raft, Some(&clearance))?;
        if parts.base_mesh.is_none() && parts.footprint.is_empty() {
            continue;
        }
        let mut meshes = Vec::new();
        if let Some(base) = parts.base_mesh {
            meshes.push(base);
        }
        if raft.bottom_mode == RaftBottomMode::Line {
            let nodes2d: Vec<Vector2> = roots.iter().map(|r| Vector2::new(r.x, r.y)).collect();
            let edge_pairs = build_line_raft_edge_pairs(
                &nodes2d,
                BuildLineRaftEdgePairsOptions {
                    has_border_ring: !parts.footprint.is_empty(),
                    keep_factor: Some(8.0),
                    abs_max_len: Some(220.0),
                    enforce_connected: Some(true),
                    include_spanning_tree_backbone: None,
                },
            );
            let beam_height = js_max(0.01, raft.line_height_mm);
            let pairs: Vec<(Vector2, Vector2)> = edge_pairs
                .iter()
                .map(|&(a, b)| (nodes2d[a], nodes2d[b]))
                .collect();
            let inflated = inflate_model_plate_clearance(&clearance)?;
            let union_edges = filter_line_raft_edges(&pairs, Some(&inflated), raft.line_width_mm);
            let union_mesh = generate_unioned_line_raft_mesh(
                &union_edges,
                raft.line_width_mm,
                beam_height,
                None,
                None,
            )?;
            if union_mesh.vertex_count() > 0 {
                meshes.push(union_mesh);
            } else {
                for (a, b) in &union_edges {
                    meshes.push(generate_chamfered_beam(
                        Vec3::new(a.x, a.y, 0.0),
                        Vec3::new(b.x, b.y, 0.0),
                        raft.line_width_mm,
                        beam_height,
                        90.0,
                    ));
                }
            }
        }
        if let Some(wall) = parts.wall_mesh {
            meshes.push(wall);
        }
        groups.push(RaftGroup { model_key, meshes });
    }
    Ok(groups)
}
