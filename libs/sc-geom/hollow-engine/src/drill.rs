//! Drains drilled into a mesh as it is (#447): no cavity is computed.
//!
//! [`crate::Session::apply`] cuts drains into the hollow it builds. A
//! hollow that came in a file (no record of how it was made) needs no new
//! cavity, and must not get one: the mesh already has one. [`DrillTarget`] reads it off the mesh, along each
//! drain's axis: the outer face, the inner wall, and the far side of the
//! air behind it. That air must be a cavity's ([`crate::air`]: where a ball
//! rolled in from outside does not reach, as it reaches into the gap
//! between a leg and the body but not through a drain) and closed in (rays
//! from it in every direction meet the mesh), so a solid model, or the gap
//! between an arm and the body, is never drilled.
//!
//! The cut is the one apply makes: the same prism or cone, the same depth
//! rule (#315, #316), the same plug (#318), and infill members a drain
//! reaches into go whole (#339). The rest of the mesh is left as it is.

use ahash::AHashMap;

use crate::air::AirMap;
use crate::bvh::Bvh;
use crate::control::{Control, Stage};
use crate::drain::{append, intersect_convex_closed, subtract_convex_closed};
use crate::engine::{
    DRAIN_RAY_START_MM, DRAIN_SAME_SURFACE_MM, DrainHole, DrainPlug, DrainReport, HollowError,
    clear_channels, drain_axis, drain_cutter, drain_segments, infill_cutter, opening_across,
    plug_cutter, report_of_opening,
};
use crate::mesh::{IndexedMesh, Vec3};
use crate::topology::{EdgeManifoldDefects, edge_components, edge_manifold_defects};

/// Rays cast from the air behind a wall to tell a closed cavity from the
/// outside.
const ENCLOSURE_RAYS: usize = 48;
/// How many of them may leave without meeting the mesh: through a drain
/// already drilled, or by slipping between two triangles.
const ENCLOSURE_ESCAPES: usize = 2;
/// How far from the surface a drain's centre may be: it is drilled where
/// it was clicked.
const ON_SURFACE_MM: f32 = 0.5;

/// A mesh to drill drains into, with what measuring them needs.
#[derive(Debug)]
pub struct DrillTarget {
    mesh: IndexedMesh,
    bvh: Bvh,
    defects: EdgeManifoldDefects,
    /// Whether each face is on a shell facing out (the model's outside, an
    /// infill member), not a cavity's wall.
    outward: Vec<bool>,
    /// Cavities closed all round: shells facing in.
    sealed: usize,
    /// Which air is a cavity's.
    air: AirMap,
}

/// The mesh after [`DrillTarget::drill`].
#[derive(Debug, Clone)]
pub struct Drilled {
    /// The mesh with the drains that open cut, edge-manifold.
    pub mesh: IndexedMesh,
    /// One per drain given, in order.
    pub drains: Vec<DrainReport>,
    /// One per drain cut that asked for a plug, in drain order.
    pub plugs: Vec<DrainPlug>,
}

impl DrillTarget {
    pub fn new(mesh: IndexedMesh) -> Self {
        let bvh = Bvh::build(&mesh);
        let defects = edge_manifold_defects(&mesh);
        let (labels, count) = edge_components(&mesh);
        let volumes = shell_volumes(&mesh, &labels, count);
        let outward = labels.iter().map(|&l| volumes[l as usize] >= 0.0).collect();
        let sealed = volumes.iter().filter(|&&v| v < 0.0).count();
        let air = AirMap::new(&mesh, &bvh);
        Self {
            mesh,
            bvh,
            defects,
            outward,
            sealed,
            air,
        }
    }

    /// How many cavities are closed all round, with no drain: each traps
    /// the resin in it (#130).
    pub fn sealed_cavities(&self) -> usize {
        self.sealed
    }

    /// Whether face `f` is on a shell facing out (not a cavity's wall).
    pub fn faces_out(&self, f: usize) -> bool {
        self.outward.get(f).copied().unwrap_or(false)
    }

    /// How far into the mesh along `inward` from `point` (on its surface)
    /// a cavity starts, when one closed in (by `rays` rays, all but a few
    /// meeting the mesh) is right behind the wall there: the wall's
    /// thickness. A cheaper look than [`DrillTarget::report`], for a map.
    pub fn wall_depth(&self, point: [f32; 3], inward: [f32; 3], rays: usize) -> Option<f32> {
        let axis = Vec3::new(inward[0], inward[1], inward[2]).normalized()?;
        let start = Vec3::new(point[0], point[1], point[2]).sub(axis.scale(DRAIN_RAY_START_MM));
        let walls = self.walls(start, axis);
        if walls.len() < 3 || walls.len() % 2 == 1 || walls[0] > DRAIN_RAY_START_MM + ON_SURFACE_MM
        {
            return None;
        }
        self.cavity_behind(start, axis, (walls[1], walls[2]), rays)
            .then_some(walls[1] - DRAIN_RAY_START_MM)
    }

    pub fn mesh(&self) -> &IndexedMesh {
        &self.mesh
    }

    /// Whether the mesh is closed and edge-manifold, as a cut needs.
    pub fn is_closed(&self) -> bool {
        self.defects.is_clean()
    }

    /// What `hole` would do: whether it opens into a closed cavity of the
    /// mesh, and how deep it goes, as [`crate::Session::drain_report`]
    /// measures it against a cavity it built.
    pub fn report(&self, hole: &DrainHole) -> DrainReport {
        let segments = drain_segments(hole);
        report_of_opening(hole, 0.0, self.opening(hole, segments), segments)
    }

    /// Where `hole` opens into a closed cavity, if its axis reaches one
    /// right through the wall it starts on.
    fn opening(&self, hole: &DrainHole, segments: usize) -> Option<crate::engine::DrainOpening> {
        let (center, axis) = drain_axis(hole);
        let start = center.sub(axis.scale(DRAIN_RAY_START_MM));
        // Where a ray from `offset` across the axis leaves the solid into
        // air and meets solid again, from the start plane.
        let span = |offset: Vec3| -> Option<(f32, f32)> {
            let walls = self.walls(start.add(offset), axis);
            // An odd count: the ray starts in the solid (a surface that
            // curves out past the start plane), so its first wall is the
            // one into the air.
            let first = usize::from(walls.len().is_multiple_of(2));
            let enter = *walls.get(first)?;
            Some((
                enter,
                walls.get(first + 1).copied().unwrap_or(f32::INFINITY),
            ))
        };
        let walls = self.walls(start, axis);
        // The axis starts outside, enters the wall, leaves it into air and
        // meets the far side of that air.
        if walls.len() < 3 || walls.len() % 2 == 1 || walls[0] > DRAIN_RAY_START_MM + ON_SURFACE_MM
        {
            return None;
        }
        let (to_cavity, cavity_end) = (walls[1], walls[2]);
        if !self.cavity_behind(start, axis, (to_cavity, cavity_end), ENCLOSURE_RAYS) {
            return None;
        }
        Some(opening_across(
            hole,
            axis,
            segments,
            (to_cavity, cavity_end),
            span,
        ))
    }

    /// The surfaces a ray from `origin` along `dir` crosses, one distance
    /// per surface.
    fn walls(&self, origin: Vec3, dir: Vec3) -> Vec<f32> {
        let mut walls: Vec<f32> = Vec::new();
        for t in self.bvh.ray_hit_distances_closed(&self.mesh, origin, dir) {
            if walls
                .last()
                .is_none_or(|&last| t - last > DRAIN_SAME_SURFACE_MM)
            {
                walls.push(t);
            }
        }
        walls
    }

    /// Whether the air from `near` to `far` along the ray from `start` along
    /// `axis` is a cavity's: past the reach of a ball from outside, and
    /// closed in, by `rays` rays from its middle.
    fn cavity_behind(&self, start: Vec3, axis: Vec3, (near, far): (f32, f32), rays: usize) -> bool {
        let (a, b) = (start.add(axis.scale(near)), start.add(axis.scale(far)));
        self.air.cavity_along(a, b) && self.closed_in_by(a.add(b).scale(0.5), rays)
    }

    /// Whether the air at `p` is closed in by the mesh: rays in `rays`
    /// directions meet it, all but [`ENCLOSURE_ESCAPES`].
    fn closed_in_by(&self, p: Vec3, rays: usize) -> bool {
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        let mut escapes = 0;
        for i in 0..rays {
            // Evenly over the sphere, none along an axis.
            let z = 1.0 - (2.0 * i as f32 + 1.0) / rays as f32;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let a = golden * i as f32 + 0.1;
            let dir = Vec3::new(r * a.cos(), r * a.sin(), z);
            if self.bvh.ray_hit_count(&self.mesh, p, dir) == 0 {
                escapes += 1;
                if escapes > ENCLOSURE_ESCAPES {
                    return false;
                }
            }
        }
        true
    }

    /// Cuts every drain of `holes` that opens into a cavity, each measured
    /// on the mesh as it is before any is cut, so drains do not depend on
    /// their order. A drain that does not open is reported and left uncut.
    pub fn drill(&self, holes: &[DrainHole], ctl: Control<'_>) -> Result<Drilled, HollowError> {
        if !self.defects.is_clean() {
            return Err(HollowError::NotClosed(self.defects));
        }
        ctl.stage(Stage::Drains)?;
        let mut reports = Vec::with_capacity(holes.len());
        let mut cutters = Vec::with_capacity(holes.len());
        for (index, hole) in holes.iter().enumerate() {
            ctl.check()?;
            let report = self.report(hole);
            if report.reaches_cavity {
                cutters.push((index, drain_cutter(hole, &report)));
            }
            reports.push(report);
        }
        if cutters.is_empty() {
            return Ok(Drilled {
                mesh: self.mesh.clone(),
                drains: reports,
                plugs: Vec::new(),
            });
        }
        let parts = Parts::of(&self.mesh);
        let body = &parts.body;
        let mut plugs = Vec::new();
        for (index, _) in &cutters {
            let hole = &holes[*index];
            if let Some(clearance) = hole.plug_clearance_mm {
                ctl.check()?;
                let cutter = plug_cutter(hole, &reports[*index], clearance);
                plugs.push(DrainPlug {
                    drain: *index,
                    mesh: intersect_convex_closed(body, &cutter)
                        .ok()
                        .filter(|plug| !plug.triangles.is_empty()),
                });
            }
        }
        let mut mesh = body.clone();
        for (index, cutter) in &cutters {
            ctl.check()?;
            mesh = subtract_convex_closed(&mesh, cutter).map_err(|error| HollowError::Drain {
                index: *index,
                error,
            })?;
        }
        if let Some((members, wall)) = &parts.infill {
            ctl.check()?;
            let channels: Vec<_> = cutters
                .iter()
                .map(|(index, _)| (*index, infill_cutter(&holes[*index], &reports[*index])))
                .collect();
            let wall_bvh = Bvh::build(wall);
            append(
                &mut mesh,
                &clear_channels(members, &channels, (wall, &wall_bvh)),
            );
        }
        ctl.stage(Stage::Validate)?;
        Ok(Drilled {
            mesh,
            drains: reports,
            plugs,
        })
    }
}

/// A hollow split for cutting: the infill members (pillars, struts: closed
/// shells inside a cavity, overlapping its wall and one another) apart from
/// the body (the outer shell and the cavity walls), which a drain cuts as
/// one solid.
struct Parts {
    body: IndexedMesh,
    /// The members, and the cavity walls they stand in.
    infill: Option<(IndexedMesh, IndexedMesh)>,
}

impl Parts {
    fn of(mesh: &IndexedMesh) -> Self {
        let (labels, count) = edge_components(mesh);
        if count < 3 {
            // An outer shell and at most one cavity: no members.
            return Self {
                body: mesh.clone(),
                infill: None,
            };
        }
        let shells = shells_of(mesh, &labels, count);
        let volumes = shell_volumes(mesh, &labels, count);
        let boxes: Vec<_> = shells.iter().map(IndexedMesh::bbox).collect();
        // A member: a positive shell centred in a cavity, smaller than it.
        let member = |s: usize| {
            volumes[s] > 0.0
                && (0..count).any(|c| {
                    let (b, cavity) = (&boxes[s], &boxes[c]);
                    let mid = b.center();
                    volumes[c] < 0.0
                        && volumes[s] < -volumes[c]
                        && (0..3).all(|k| {
                            let (lo, hi, m) = match k {
                                0 => (cavity.min.x, cavity.max.x, mid.x),
                                1 => (cavity.min.y, cavity.max.y, mid.y),
                                _ => (cavity.min.z, cavity.max.z, mid.z),
                            };
                            (lo..=hi).contains(&m)
                        })
                })
        };
        let is_member: Vec<bool> = (0..count).map(member).collect();
        if !is_member.contains(&true) {
            return Self {
                body: mesh.clone(),
                infill: None,
            };
        }
        let mut body = IndexedMesh::default();
        let mut members = IndexedMesh::default();
        let mut wall = IndexedMesh::default();
        for (s, shell) in shells.iter().enumerate() {
            if is_member[s] {
                append(&mut members, shell);
            } else {
                append(&mut body, shell);
                if volumes[s] < 0.0 {
                    append(&mut wall, shell);
                }
            }
        }
        Self {
            body,
            infill: Some((members, wall)),
        }
    }
}

/// How many cavities of `mesh` are closed all round (shells facing in): each
/// traps resin until a drain opens it (#130).
pub fn sealed_cavities(mesh: &IndexedMesh) -> usize {
    let (labels, count) = edge_components(mesh);
    shell_volumes(mesh, &labels, count)
        .iter()
        .filter(|&&v| v < 0.0)
        .count()
}

/// The signed volume of each shell of `mesh` (its faces by `labels`).
fn shell_volumes(mesh: &IndexedMesh, labels: &[u32], count: usize) -> Vec<f64> {
    let mut volumes = vec![0.0f64; count];
    for (tri, &label) in mesh.triangles.iter().zip(labels) {
        let [a, b, c] = tri.map(|v| mesh.positions[v as usize]);
        let (a, b, c) = (
            [a.x, a.y, a.z].map(f64::from),
            [b.x, b.y, b.z].map(f64::from),
            [c.x, c.y, c.z].map(f64::from),
        );
        volumes[label as usize] += (a[0] * (b[1] * c[2] - b[2] * c[1])
            - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]))
            / 6.0;
    }
    volumes
}

/// Each shell of `mesh` (its faces by `labels`, `count` of them) as a mesh
/// of its own, in one pass.
fn shells_of(mesh: &IndexedMesh, labels: &[u32], count: usize) -> Vec<IndexedMesh> {
    let mut shells = vec![IndexedMesh::default(); count];
    let mut local: AHashMap<(u32, u32), u32> = AHashMap::new();
    for (tri, &label) in mesh.triangles.iter().zip(labels) {
        let shell = &mut shells[label as usize];
        let face = tri.map(|v| {
            *local.entry((label, v)).or_insert_with(|| {
                shell.positions.push(mesh.positions[v as usize]);
                (shell.positions.len() - 1) as u32
            })
        });
        shell.triangles.push(face);
    }
    shells
}

#[cfg(test)]
#[path = "drill_tests.rs"]
mod tests;
