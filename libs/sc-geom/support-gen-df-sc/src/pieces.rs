//! The support graph as data (#432): every piece's line and where it touches
//! the model, in world mm, as the slicer draws it.
//!
//! Nothing is sliced or meshed to read these, so what they cost grows with
//! the number of pieces and never with their geometry. The review reads
//! them instead of the supports' contours.

use support_gen_df::features::slicing::support_world_triangles::{
    TipDrawing, contact_disk_size, contact_ends, resolve_support_slice_tessellation,
};
use support_gen_df::supports::support_primitives::contact_cone::types::ContactCone;
use support_gen_df::supports::types::{ContactDisk, Segment};
use support_gen_df::three::Vec3;

use super::DfSupportSession;

/// What a [`SupportPiece`] is in the support graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SupportPieceKind {
    /// A column from a root on the plate or the raft.
    Trunk,
    /// A column from a knot on another support.
    Branch,
    /// A tip on a knot, with no shaft of its own.
    Leaf,
    /// A thin bar between two contact disks on the model.
    Twig,
    /// A bar between two contact cones on the model.
    Stick,
    /// A short support near the plate, with its own foot.
    Stump,
}

/// A point of a support's line, in world mm, and its diameter there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SupportJoint {
    /// Where it is.
    pub at: [f64; 3],
    /// The support's diameter there, 0 when the graph does not say.
    pub diameter_mm: f64,
}

/// Where a support touches the model, as the slicer draws it (world mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SupportContact {
    /// The contact point on the model's surface.
    pub surface: [f64; 3],
    /// The unit direction from `surface` towards the socket.
    pub axis: [f64; 3],
    /// How far the tip goes into the model past `surface`.
    pub inserted_mm: f64,
    /// Where the tip ends inside the model.
    pub tip: [f64; 3],
    /// Its radius there. Tips are drawn with a few flat sides: this is the
    /// radius of the circle with the area they enclose, and so are the other
    /// radii here.
    pub tip_radius_mm: f64,
    /// The top of its break-point neck, a cylinder of the tip's radius, when
    /// it has one.
    pub neck: Option<[f64; 3]>,
    /// Where it meets the support's shaft.
    pub socket: [f64; 3],
    /// Its radius there.
    pub socket_radius_mm: f64,
}

impl SupportContact {
    fn of_cone(
        cone: &ContactCone,
        penetration_mm: f64,
        tip_scale: f64,
        drawing: TipDrawing,
        sides: f64,
    ) -> Self {
        let ends = contact_ends(cone, penetration_mm, tip_scale, drawing);
        let length = cone.pos.distance_to(ends.socket);
        let axis = if length > 1e-9 {
            ends.socket.sub(cone.pos).multiply_scalar(1.0 / length)
        } else {
            cone.normal
        };
        Self {
            surface: at(cone.pos),
            axis: at(axis),
            inserted_mm: cone.pos.distance_to(ends.tip),
            tip: at(ends.tip),
            tip_radius_mm: ends.r_tip * sides,
            neck: ends.neck.map(at),
            socket: at(ends.socket),
            socket_radius_mm: ends.r_socket * sides,
        }
    }

    /// A contact disk, read as a cylinder from inside the model to the top
    /// of the ball that closes it.
    fn of_disk(disk: &ContactDisk, penetration_mm: f64, tip_scale: f64, sides: f64) -> Self {
        let (thickness, radius) = contact_disk_size(disk, tip_scale);
        let n = disk.surface_normal;
        Self {
            surface: at(disk.pos),
            axis: at(n),
            inserted_mm: penetration_mm,
            tip: at(disk.pos.sub(n.multiply_scalar(penetration_mm))),
            tip_radius_mm: radius * sides,
            neck: None,
            socket: at(disk.pos.add(n.multiply_scalar(thickness + radius))),
            socket_radius_mm: radius * sides,
        }
    }
}

/// One piece of the support graph (a trunk, a branch, a leaf, a twig, a
/// stick or a stump) as data: see [`DfSupportSession::pieces`].
#[derive(Clone, Debug, PartialEq)]
pub struct SupportPiece {
    /// The piece's id in the graph.
    pub id: String,
    /// What it is.
    pub kind: SupportPieceKind,
    /// Its joints in the order of its segments, then the root, knot or foot
    /// it stands on. Empty only for a leaf whose knot is gone.
    pub line: Vec<SupportJoint>,
    /// Where it touches the model: one contact, two for a bar between two
    /// parts of the model, none for a trunk or a branch that stops short.
    pub contacts: Vec<SupportContact>,
}

fn at(p: Vec3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

/// The radius of the circle as large as a regular polygon of `sides` sides
/// inside a circle of radius 1.
fn equal_area_radius(sides: u32) -> f64 {
    let sides = f64::from(sides.max(3));
    (sides / std::f64::consts::TAU * (std::f64::consts::TAU / sides).sin()).sqrt()
}

/// The joints of `segments` in order, each once, with the segment's
/// diameter where it is wider, then `end`.
fn line_of(segments: &[Segment], end: Option<SupportJoint>) -> Vec<SupportJoint> {
    let mut line: Vec<SupportJoint> = Vec::with_capacity(segments.len() + 2);
    for segment in segments {
        for joint in [segment.top_joint.as_ref(), segment.bottom_joint.as_ref()]
            .into_iter()
            .flatten()
        {
            let point = SupportJoint {
                at: at(joint.pos),
                diameter_mm: joint.diameter.max(segment.diameter),
            };
            if line.last().is_none_or(|last| last.at != point.at) {
                line.push(point);
            }
        }
    }
    line.extend(end);
    line
}

impl DfSupportSession {
    /// Every piece of the support graph as data: its line and where it
    /// touches the model. Trunks, branches and leaves hold the model from
    /// below; sticks and twigs join two parts of it, as they do inside a
    /// hollow. Braces and kickstands touch no model and are left out.
    pub fn pieces(&self) -> Vec<SupportPiece> {
        let c = &self.state.collections;
        let settings = &self.settings;
        let tip_scale = 1.0 - settings.support_tip_shrink_percent / 100.0;
        let penetration_mm = settings.tip_penetration_mm;
        let sides =
            equal_area_radius(resolve_support_slice_tessellation(c).contact_cone_radial_segments);
        let cone = |cone: &ContactCone| {
            SupportContact::of_cone(cone, penetration_mm, tip_scale, settings.tip_drawing, sides)
        };
        let disk =
            |disk: &ContactDisk| SupportContact::of_disk(disk, penetration_mm, tip_scale, sides);
        let knot = |id: &str| {
            c.knots.get(id).map(|k| SupportJoint {
                at: at(k.pos),
                diameter_mm: k.diameter.unwrap_or(0.0),
            })
        };
        let root = |id: &str| {
            c.roots.get(id).map(|r| SupportJoint {
                at: at(r.transform.pos),
                diameter_mm: r.diameter,
            })
        };
        let mut pieces = Vec::with_capacity(
            c.trunks.len()
                + c.branches.len()
                + c.leaves.len()
                + c.twigs.len()
                + c.sticks.len()
                + c.stumps.len(),
        );
        let mut add = |id: &str, kind, line, contacts| {
            pieces.push(SupportPiece {
                id: id.to_owned(),
                kind,
                line,
                contacts,
            });
        };
        for (id, t) in &c.trunks {
            add(
                id,
                SupportPieceKind::Trunk,
                line_of(&t.segments, root(&t.root_id)),
                t.contact_cone.iter().map(cone).collect(),
            );
        }
        for (id, b) in &c.branches {
            add(
                id,
                SupportPieceKind::Branch,
                line_of(&b.segments, knot(&b.parent_knot_id)),
                b.contact_cone.iter().map(cone).collect(),
            );
        }
        for (id, l) in &c.leaves {
            add(
                id,
                SupportPieceKind::Leaf,
                line_of(&[], knot(&l.parent_knot_id)),
                vec![cone(&l.contact_cone)],
            );
        }
        for (id, t) in &c.twigs {
            add(
                id,
                SupportPieceKind::Twig,
                line_of(&t.segments, None),
                vec![disk(&t.contact_disk_a), disk(&t.contact_disk_b)],
            );
        }
        for (id, s) in &c.sticks {
            add(
                id,
                SupportPieceKind::Stick,
                line_of(&s.segments, None),
                vec![cone(&s.contact_cone_a), cone(&s.contact_cone_b)],
            );
        }
        for (id, s) in &c.stumps {
            let foot = SupportJoint {
                at: at(s.root_pos),
                diameter_mm: s.root_base_diameter,
            };
            add(
                id,
                SupportPieceKind::Stump,
                line_of(&s.segments, Some(foot)),
                vec![cone(&s.contact_cone)],
            );
        }
        pieces
    }
}
