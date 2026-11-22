/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A placed support edited part by part, in place (#377): its trunk moved
//! across the plate, an elbow moved anywhere, the widths of its tip, shaft
//! and foot. Its tip stays where it touches the model and its route is the
//! one the person leaves; what hangs from it is routed again from its own
//! tip, and the braces or kickstands on it go. An edit that would take a
//! shaft through the model, lean one past what prints, or put an elbow out
//! of order is refused, and the session is as it was.

use std::collections::HashSet;

use bread_support_geom::geom::Vec3;
use bread_support_model::tips::get_final_socket_position;
use bread_support_model::types::{Joint, Roots, Segment, SegmentShape};

use bread_support_model::auto_support::zone_rules::SupportSize;

use super::{
    AddNote, AddOptions, At, EditError, EditPreview, FORCED_MAX_ANGLE_DEG, HandShape,
    MANUAL_MAX_ANGLE_DEG, SupportSession,
};

/// How far apart the axis of a shaft is read against the model, mm.
const CLEAR_STEP_MM: f64 = 0.2;
/// The share of a shaft's radius the model may come within of its axis: a
/// facet of the drawn shaft is a little inside the round one.
const CLEAR_SHARE: f64 = 0.9;
/// How much higher than the part below it an elbow stays, mm: a shaft only
/// rises from its foot to its tip.
const MIN_RISE_MM: f64 = 0.2;
/// A moved trunk leans into its tip no more than this short of the limit,
/// degrees: an elbow made for it is not on the edge of a refusal.
const LEAN_MARGIN_DEG: f64 = 2.0;

/// The parts of a placed support, world mm (#377).
#[derive(Clone, Debug, PartialEq)]
pub struct SupportParts {
    /// Where its tip touches the model.
    pub tip: [f64; 3],
    /// Where its shaft meets the tip's cone.
    pub socket: [f64; 3],
    /// Its elbows, from the bottom up, by id: the points of its shaft
    /// between its foot (or the knot it hangs from) and its socket.
    pub elbows: Vec<(String, [f64; 3])>,
    /// Where it stands on the plate, for a trunk: its root's centre.
    pub foot: Option<[f64; 3]>,
    /// The bottom of its shaft: the top of its root, or the knot it hangs
    /// from.
    pub anchor: [f64; 3],
    /// How wide its tip is where it touches, mm.
    pub tip_mm: f64,
    /// How wide its shaft is, mm; `None` for a tip with no shaft (a leaf).
    pub trunk_mm: Option<f64>,
    /// How wide its foot is on the plate, mm, for a trunk.
    pub foot_mm: Option<f64>,
}

/// New widths for a support's parts (#377); `None` keeps that part's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartSizes {
    pub tip_mm: Option<f64>,
    pub trunk_mm: Option<f64>,
    pub foot_mm: Option<f64>,
}

/// A support of an imported set to make editable (#261).
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedPlan {
    /// Where its tip touches the model (world mm).
    pub at: [f64; 3],
    /// The numbers it is drawn with; `None` takes the preset's.
    pub shape: Option<SupportSize>,
    /// The route it took: where its foot stands across the plate (world
    /// mm) and its elbows from the bottom up. `None` for one routed as Add
    /// routes it, hung from another where one is in reach.
    pub route: Option<([f64; 2], Vec<[f64; 3]>)>,
}

/// Which collection a shaft is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShaftKind {
    Trunk,
    Branch,
}

/// A support's shaft as a route: the foot it stands on, its elbows and the
/// socket, which stays.
struct Route {
    kind: ShaftKind,
    /// The root's centre on the plate, for a trunk.
    foot: Option<Vec3>,
    /// The bottom of the shaft.
    anchor: Vec3,
    elbows: Vec<(String, Vec3)>,
    socket: Vec3,
}

fn v([x, y, z]: [f64; 3]) -> Vec3 {
    Vec3::new(x, y, z)
}

fn a(p: Vec3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

/// The top of `root`, where its trunk's shaft starts.
fn root_top(root: &Roots) -> Vec3 {
    let finite = |h: f64| if h.is_finite() { h } else { 0.0 };
    let cone = if root.cone_height.is_finite() {
        root.cone_height
    } else {
        root.height.filter(|h| h.is_finite()).unwrap_or(0.0)
    };
    root.transform
        .pos
        .add(Vec3::new(0.0, 0.0, finite(root.disk_height) + cone))
}

/// How far from the vertical the way from `from` up to `to` leans, degrees;
/// 180 for a way that does not rise.
fn lean_deg(from: Vec3, to: Vec3) -> f64 {
    let d = to.sub(from);
    if d.z <= 0.0 {
        return 180.0;
    }
    d.x.hypot(d.y).atan2(d.z).to_degrees()
}

impl SupportSession {
    /// The segments of shaft `id` (a trunk or a branch).
    fn shaft_segments(&self, id: &str) -> Option<(ShaftKind, &[Segment])> {
        let c = &self.state.collections;
        c.trunks
            .get(id)
            .map(|t| (ShaftKind::Trunk, t.segments.as_slice()))
            .or_else(|| {
                c.branches
                    .get(id)
                    .map(|b| (ShaftKind::Branch, b.segments.as_slice()))
            })
    }

    fn shaft_segments_mut(&mut self, id: &str) -> Option<&mut Vec<Segment>> {
        let c = &mut self.state.collections;
        if let Some(t) = c.trunks.get_mut(id) {
            return Some(&mut t.segments);
        }
        c.branches.get_mut(id).map(|b| &mut b.segments)
    }

    /// Support `id`'s shaft as a route; `None` for one with no shaft (a
    /// leaf) or none of these supports.
    fn route_of(&self, id: &str) -> Option<Route> {
        let c = &self.state.collections;
        let (kind, segments) = self.shaft_segments(id)?;
        let (foot, anchor, cone) = match kind {
            ShaftKind::Trunk => {
                let t = c.trunks.get(id)?;
                let root = c.roots.get(&t.root_id)?;
                (
                    Some(root.transform.pos),
                    root_top(root),
                    t.contact_cone.as_ref()?,
                )
            }
            ShaftKind::Branch => {
                let b = c.branches.get(id)?;
                (
                    None,
                    c.knots.get(&b.parent_knot_id)?.pos,
                    b.contact_cone.as_ref()?,
                )
            }
        };
        let last = segments.last()?;
        let socket = last
            .top_joint
            .as_ref()
            .map_or_else(|| get_final_socket_position(cone), |j| j.pos);
        let elbows = segments[..segments.len() - 1]
            .iter()
            .filter_map(|s| s.top_joint.as_ref().map(|j| (j.id.clone(), j.pos)))
            .collect();
        Some(Route {
            kind,
            foot,
            anchor,
            elbows,
            socket,
        })
    }

    /// The parts of support `id` (#377): its tip, elbows and foot, and how
    /// wide each is.
    pub fn parts_of(&self, id: &str) -> Option<SupportParts> {
        let (tip, _) = self.support_tip(id)?;
        let c = &self.state.collections;
        let cone = c
            .trunks
            .get(id)
            .and_then(|t| t.contact_cone.as_ref())
            .or_else(|| c.branches.get(id).and_then(|b| b.contact_cone.as_ref()))
            .or_else(|| c.leaves.get(id).map(|l| &l.contact_cone))?;
        let tip_mm = cone.profile.contact_diameter_mm;
        let Some(route) = self.route_of(id) else {
            // A leaf: a tip on a knot.
            let knot = c
                .leaves
                .get(id)
                .and_then(|l| c.knots.get(&l.parent_knot_id));
            let socket = get_final_socket_position(cone);
            return Some(SupportParts {
                tip,
                socket: a(socket),
                elbows: Vec::new(),
                foot: None,
                anchor: knot.map_or(a(socket), |k| a(k.pos)),
                tip_mm,
                trunk_mm: None,
                foot_mm: None,
            });
        };
        let (_, segments) = self.shaft_segments(id)?;
        let foot_mm = c
            .trunks
            .get(id)
            .and_then(|t| c.roots.get(&t.root_id))
            .map(|r| r.diameter);
        Some(SupportParts {
            tip,
            socket: a(route.socket),
            elbows: route
                .elbows
                .iter()
                .map(|(e, p)| (e.clone(), a(*p)))
                .collect(),
            foot: route.foot.map(a),
            anchor: a(route.anchor),
            tip_mm,
            trunk_mm: segments.first().map(|s| s.diameter),
            foot_mm,
        })
    }

    /// Moves the trunk of support `id` across the plate by `delta` (world
    /// mm, X and Y), its foot and elbows with it; its tip stays (#377). The
    /// way up into the tip leans no more than a support placed by hand may:
    /// elbows that would lean more are dropped, and one is made right above
    /// the trunk where the way needs it. On a refusal the session is as it
    /// was.
    pub fn move_trunk(&mut self, id: &str, delta: [f64; 2]) -> Result<(), EditError> {
        let moved = self.trunk_moved(id, delta)?;
        let checked = moved.checked(id)?;
        *self = checked;
        Ok(())
    }

    /// What [`Self::move_trunk`] would do: the support where it goes (green,
    /// with what it leaves in `taken`), or red there with why it cannot.
    /// With the session the move leaves when it goes through.
    pub fn preview_move_trunk(
        &self,
        id: &str,
        delta: [f64; 2],
    ) -> Result<(EditPreview, Option<Self>), EditError> {
        let moved = self.trunk_moved(id, delta);
        self.preview_part_edit(id, moved)
    }

    /// Moves elbow `elbow` of support `id` to `to` (world mm), the shaft on
    /// either side of it following (#377). It stays above the part below it
    /// and under the one above it, and neither leans past what a forced
    /// attach may. On a refusal the session is as it was.
    pub fn move_elbow(&mut self, id: &str, elbow: &str, to: [f64; 3]) -> Result<(), EditError> {
        let moved = self.elbow_moved(id, elbow, to)?;
        let checked = moved.checked(id)?;
        *self = checked;
        Ok(())
    }

    /// What [`Self::move_elbow`] would do, as [`Self::preview_move_trunk`].
    pub fn preview_move_elbow(
        &self,
        id: &str,
        elbow: &str,
        to: [f64; 3],
    ) -> Result<(EditPreview, Option<Self>), EditError> {
        let moved = self.elbow_moved(id, elbow, to);
        self.preview_part_edit(id, moved)
    }

    /// The elbow of support `id` nearest `point` (world mm), within
    /// `reach_mm`: what a drag on it moves (#377).
    pub fn elbow_near(&self, id: &str, point: [f64; 3], reach_mm: f64) -> Option<String> {
        let route = self.route_of(id)?;
        route
            .elbows
            .iter()
            .map(|(e, p)| (p.distance_to(v(point)), e))
            .filter(|(d, _)| *d <= reach_mm)
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map(|(_, e)| e.clone())
    }

    /// Gives the parts of support `id` the widths `sizes` names, its route
    /// as it is (#377): one others hang from too. A shaft made wider that
    /// would touch the model, or a tip wider than its shaft, is refused,
    /// and the session is as it was.
    pub fn resize_parts(&mut self, id: &str, sizes: PartSizes) -> Result<(), EditError> {
        let parts = self.parts_of(id).ok_or(EditError::NothingThere)?;
        let mut next = self.clone();
        let c = &mut next.state.collections;
        let shaft = sizes.trunk_mm.or(parts.trunk_mm);
        let tip = sizes.tip_mm.unwrap_or(parts.tip_mm);
        if let Some(shaft) = shaft
            && tip >= shaft
        {
            return Err(EditError::CannotAttach(AddNote::TipOverShaft));
        }
        let cone = c
            .trunks
            .get_mut(id)
            .and_then(|t| t.contact_cone.as_mut())
            .or_else(|| c.branches.get_mut(id).and_then(|b| b.contact_cone.as_mut()))
            .or_else(|| c.leaves.get_mut(id).map(|l| &mut l.contact_cone))
            .ok_or(EditError::NothingThere)?;
        if let Some(mm) = sizes.tip_mm {
            cone.profile.contact_diameter_mm = mm.max(0.05);
        }
        if let (Some(mm), Some(old)) = (sizes.trunk_mm, parts.trunk_mm) {
            cone.profile.body_diameter_mm = mm;
            let grow = mm - old;
            let mut on_it = HashSet::new();
            if let Some(segments) = next.shaft_segments_mut(id) {
                for s in segments.iter_mut() {
                    on_it.insert(s.id.clone());
                    s.diameter = mm;
                    for j in [s.top_joint.as_mut(), s.bottom_joint.as_mut()]
                        .into_iter()
                        .flatten()
                    {
                        j.diameter = (j.diameter + grow).max(mm);
                    }
                }
            }
            let c = &mut next.state.collections;
            if let Some(t) = c.trunks.get_mut(id)
                && t.base_diameter_mm.is_some()
            {
                t.base_diameter_mm = Some(mm);
            }
            for k in c.knots.values_mut() {
                if on_it.contains(&k.parent_shaft_id)
                    && let Some(d) = k.diameter.as_mut()
                {
                    *d = (*d + grow).max(mm);
                }
            }
        }
        if let Some(mm) = sizes.foot_mm {
            let c = &mut next.state.collections;
            let root = c
                .trunks
                .get(id)
                .map(|t| t.root_id.clone())
                .and_then(|r| c.roots.get_mut(&r));
            if let Some(root) = root {
                let shaft = shaft.unwrap_or(0.0);
                root.diameter = mm.max(shaft);
            }
        }
        next.removal_index = Default::default();
        next.hand_edited = true;
        if sizes
            .trunk_mm
            .zip(parts.trunk_mm)
            .is_some_and(|(n, o)| n > o)
        {
            next.shaft_clear(id)?;
        }
        *self = next;
        Ok(())
    }

    /// Gives the trunk of support `id` the route of `foot` (across the
    /// plate, world mm) and `elbows` (from the bottom up, world mm), its tip
    /// where it is (#261). Elbows lower than its root's top go; the rest
    /// must rise to the tip, lean no more than a forced attach may and keep
    /// off the model. On a refusal the session is as it was.
    pub fn set_route(
        &mut self,
        id: &str,
        foot: [f64; 2],
        elbows: &[[f64; 3]],
    ) -> Result<(), EditError> {
        let route = self.route_of(id).ok_or(EditError::NothingThere)?;
        let (Some(old_foot), ShaftKind::Trunk) = (route.foot, route.kind) else {
            return Err(EditError::NothingThere);
        };
        let d = Vec3::new(foot[0] - old_foot.x, foot[1] - old_foot.y, 0.0);
        let anchor = route.anchor.add(d);
        let kept: Vec<(Option<String>, Vec3)> = elbows
            .iter()
            .map(|p| v(*p))
            .filter(|p| p.z >= anchor.z + MIN_RISE_MM && p.z <= route.socket.z - MIN_RISE_MM)
            .map(|p| (None, p))
            .collect();
        let mut way = vec![anchor];
        way.extend(kept.iter().map(|(_, p)| *p));
        way.push(route.socket);
        if way
            .windows(2)
            .any(|w| lean_deg(w[0], w[1]) > FORCED_MAX_ANGLE_DEG)
        {
            return Err(EditError::CannotAttach(AddNote::Leans));
        }
        let mut next = self.clone();
        next.reroute_shaft(id, Some(old_foot.add(d)), &kept);
        *self = next.checked(id)?;
        Ok(())
    }

    /// Adds a support of an imported set (#261): at its contact, drawn with
    /// its numbers and, when it took a route of its own, on that route as
    /// far as it is one a support may take here; else as Add routes it.
    /// Returns whether the route was kept.
    pub fn add_imported(&mut self, plan: &ImportedPlan) -> Result<bool, EditError> {
        // A tip shorter than a support made here has brings its shaft too
        // near the model for a route: it takes the length of the shipped
        // size nearest its width, and with no luck the preset's numbers.
        let longer = plan.shape.clone().and_then(|mut shape| {
            let shipped = crate::zone_presets::support_sizes()
                .into_iter()
                .min_by(|a, b| {
                    (a.tip_diameter_mm - shape.tip_diameter_mm)
                        .abs()
                        .total_cmp(&(b.tip_diameter_mm - shape.tip_diameter_mm).abs())
                })?;
            (shipped.tip_length_mm > shape.tip_length_mm).then(|| {
                shape.tip_length_mm = shipped.tip_length_mm;
                shape
            })
        });
        let mut tries = vec![plan.shape.clone()];
        tries.extend(longer.map(Some));
        if plan.shape.is_some() {
            tries.push(None);
        }
        let mut next = self.clone();
        let mut placed = Err(EditError::NothingThere);
        for shape in tries {
            let how = AddOptions {
                attach: false,
                size: None,
                shape: shape.map(HandShape),
            };
            placed = next
                .place_at_as(At::Point(plan.at), &how, plan.route.is_some())
                .map_err(|(err, _)| err);
            if placed.is_ok() {
                break;
            }
        }
        let placed = placed?;
        let mut kept = false;
        if let Some((foot, elbows)) = &plan.route
            && let Some(id) = placed
                .added
                .iter()
                .find(|id| next.state.collections.trunks.contains_key(*id))
                .cloned()
        {
            let mut routed = next.clone();
            if routed.set_route(&id, *foot, elbows).is_ok() {
                next = routed;
                kept = true;
            }
        }
        *self = next;
        Ok(kept)
    }

    /// The session with support `id`'s trunk moved by `delta`, unchecked.
    fn trunk_moved(&self, id: &str, delta: [f64; 2]) -> Result<Self, EditError> {
        let route = self.route_of(id).ok_or(EditError::NothingThere)?;
        if route.kind != ShaftKind::Trunk {
            return Err(EditError::NothingThere);
        }
        let d = Vec3::new(delta[0], delta[1], 0.0);
        let anchor = route.anchor.add(d);
        let mut elbows: Vec<(Option<String>, Vec3)> = route
            .elbows
            .into_iter()
            .map(|(e, p)| (Some(e), p.add(d)))
            .collect();
        let limit = MANUAL_MAX_ANGLE_DEG - LEAN_MARGIN_DEG;
        // Elbows the way into the tip would lean too far from go; one right
        // above what is left takes their place where it is needed.
        while elbows
            .last()
            .is_some_and(|(_, p)| lean_deg(*p, route.socket) > MANUAL_MAX_ANGLE_DEG)
        {
            elbows.pop();
        }
        let top = elbows.last().map_or(anchor, |(_, p)| *p);
        if lean_deg(top, route.socket) > MANUAL_MAX_ANGLE_DEG {
            let across = (route.socket.x - top.x).hypot(route.socket.y - top.y);
            let z = route.socket.z - across / limit.to_radians().tan();
            if z < top.z + MIN_RISE_MM {
                return Err(EditError::CannotAttach(AddNote::Leans));
            }
            elbows.push((None, Vec3::new(top.x, top.y, z)));
        }
        let mut next = self.clone();
        next.reroute_shaft(id, route.foot.map(|f| f.add(d)), &elbows);
        Ok(next)
    }

    /// The session with elbow `elbow` of support `id` moved to `to`,
    /// unchecked but for its order.
    fn elbow_moved(&self, id: &str, elbow: &str, to: [f64; 3]) -> Result<Self, EditError> {
        let route = self.route_of(id).ok_or(EditError::NothingThere)?;
        let k = route
            .elbows
            .iter()
            .position(|(e, _)| e == elbow)
            .ok_or(EditError::NothingThere)?;
        let to = v(to);
        let below = if k == 0 {
            route.anchor
        } else {
            route.elbows[k - 1].1
        };
        let above = route.elbows.get(k + 1).map_or(route.socket, |(_, p)| *p);
        let elbows: Vec<(Option<String>, Vec3)> = route
            .elbows
            .iter()
            .enumerate()
            .map(|(i, (e, p))| (Some(e.clone()), if i == k { to } else { *p }))
            .collect();
        let mut next = self.clone();
        next.reroute_shaft(id, route.foot, &elbows);
        if to.z < below.z + MIN_RISE_MM || to.z > above.z - MIN_RISE_MM {
            return Err(EditError::CannotAttach(AddNote::OutOfOrder));
        }
        if lean_deg(below, to) > FORCED_MAX_ANGLE_DEG || lean_deg(to, above) > FORCED_MAX_ANGLE_DEG
        {
            return Err(EditError::CannotAttach(AddNote::Leans));
        }
        Ok(next)
    }

    /// Gives shaft `id` the route through `elbows` (from the bottom up; an
    /// elbow with no id is a new one), standing on a root at `foot` for a
    /// trunk. The knots on it keep their height on the new route.
    fn reroute_shaft(&mut self, id: &str, foot: Option<Vec3>, elbows: &[(Option<String>, Vec3)]) {
        let new_ids: Vec<String> = {
            let mut ids = self.id_source();
            elbows
                .iter()
                .flat_map(|_| [ids(), ids()])
                .collect::<Vec<_>>()
        };
        let mut fresh = new_ids.into_iter();
        let c = &mut self.state.collections;
        if let (Some(foot), Some(t)) = (foot, c.trunks.get(id)) {
            let root = t.root_id.clone();
            if let Some(r) = c.roots.get_mut(&root) {
                r.transform.pos = foot;
            }
        }
        let anchor = match c.trunks.get(id) {
            Some(t) => c.roots.get(&t.root_id).map(root_top),
            None => c
                .branches
                .get(id)
                .and_then(|b| c.knots.get(&b.parent_knot_id))
                .map(|k| k.pos),
        };
        let Some(anchor) = anchor else {
            return;
        };
        let Some(segments) = self.shaft_segments_mut(id) else {
            return;
        };
        let old = std::mem::take(segments);
        let Some(last) = old.last().cloned() else {
            return;
        };
        let joint_extra = old
            .iter()
            .find_map(|s| s.top_joint.as_ref().map(|j| j.diameter - s.diameter))
            .unwrap_or(0.1)
            .max(0.0);
        let straight = SegmentShape::Straight;
        let mut joints: Vec<Joint> = Vec::with_capacity(elbows.len());
        for (e, p) in elbows {
            let id = e
                .clone()
                .unwrap_or_else(|| fresh.next().unwrap_or_default());
            joints.push(Joint {
                id,
                pos: *p,
                diameter: last.diameter + joint_extra,
            });
        }
        let mut next = Vec::with_capacity(joints.len() + 1);
        for i in 0..=joints.len() {
            let mut s = match old.get(i) {
                Some(s) if i < joints.len() => s.clone(),
                _ if i == joints.len() => last.clone(),
                _ => Segment::straight(fresh.next().unwrap_or_default(), last.diameter),
            };
            s.shape = straight;
            s.bottom_joint = i.checked_sub(1).map(|b| joints[b].clone());
            if i < joints.len() {
                s.top_joint = Some(joints[i].clone());
            }
            next.push(s);
        }
        // The last segment keeps its id and its socket; a segment id the
        // route took from an earlier one is not used twice.
        let mut seen = HashSet::new();
        for s in &mut next {
            if !seen.insert(s.id.clone()) {
                s.id = fresh.next().unwrap_or_default();
                seen.insert(s.id.clone());
            }
        }
        *segments = next;
        let route: Vec<(String, Vec3, Vec3)> = {
            let mut from = anchor;
            segments
                .iter()
                .map(|s| {
                    let to = s.top_joint.as_ref().map_or(from, |j| j.pos);
                    let span = (s.id.clone(), from, to);
                    from = to;
                    span
                })
                .collect()
        };
        let old_ids: HashSet<String> = old.iter().map(|s| s.id.clone()).collect();
        for k in self.state.collections.knots.values_mut() {
            if !old_ids.contains(&k.parent_shaft_id) {
                continue;
            }
            let z = k.pos.z;
            let span = route
                .iter()
                .find(|(_, from, to)| z >= from.z && z <= to.z)
                .or_else(|| {
                    if route.first().is_some_and(|(_, f, _)| z < f.z) {
                        route.first()
                    } else {
                        route.last()
                    }
                });
            if let Some((sid, from, to)) = span {
                let t = if to.z - from.z > 1e-9 {
                    ((z - from.z) / (to.z - from.z)).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                k.parent_shaft_id = sid.clone();
                k.t = Some(t);
                k.pos = from.lerp(*to, t);
            }
        }
        self.removal_index = Default::default();
        self.hand_edited = true;
    }

    /// `self` (support `id` just edited) with what hangs from it routed
    /// again from its tip and the braces on it gone, and its shaft checked
    /// against the model.
    fn checked(mut self, id: &str) -> Result<Self, EditError> {
        self.shaft_clear(id)?;
        let (hangers, others) = self.hung_on(id);
        if !others.is_empty() {
            let gone = self.removal_of(&others)?;
            self.log_removal(&others, &gone);
            let c = &self.state.collections;
            self.state.collections = super::filtered(c, |e| !gone.contains(e));
            self.removal_index = Default::default();
        }
        for hanger in hangers {
            let tip = self.tip_point(&hanger).ok_or(EditError::NothingThere)?;
            self.move_in_place_as(&hanger, At::Point(tip), &AddOptions::default(), false)
                .map_err(|(err, _)| err)?;
        }
        Ok(self)
    }

    /// What hangs from support `id`'s shaft: the supports (to route again)
    /// and the rest (braces, kickstands: to take off).
    fn hung_on(&self, id: &str) -> (Vec<String>, Vec<String>) {
        let c = &self.state.collections;
        let Some((_, segments)) = self.shaft_segments(id) else {
            return (Vec::new(), Vec::new());
        };
        let on: HashSet<&str> = segments.iter().map(|s| s.id.as_str()).collect();
        let knots: HashSet<&str> = c
            .knots
            .values()
            .filter(|k| on.contains(k.parent_shaft_id.as_str()))
            .map(|k| k.id.as_str())
            .collect();
        let hangers = c
            .branches
            .iter()
            .filter(|(_, b)| knots.contains(b.parent_knot_id.as_str()))
            .map(|(e, _)| e.clone())
            .chain(
                c.leaves
                    .iter()
                    .filter(|(_, l)| knots.contains(l.parent_knot_id.as_str()))
                    .map(|(e, _)| e.clone()),
            )
            .collect();
        let index = self.removal_index();
        let mut others: Vec<String> = knots
            .iter()
            .flat_map(|k| index.named_by.get(*k).into_iter().flatten())
            .map(|&i| index.ids[i].clone())
            .filter(|e| c.braces.contains_key(e) || c.kickstands.contains_key(e))
            .collect();
        others.sort();
        others.dedup();
        (hangers, others)
    }

    /// Whether support `id`'s shaft keeps off the model by its radius.
    fn shaft_clear(&self, id: &str) -> Result<(), EditError> {
        let route = self.route_of(id).ok_or(EditError::NothingThere)?;
        let (_, segments) = self.shaft_segments(id).ok_or(EditError::NothingThere)?;
        let mut from = route.anchor;
        for s in segments {
            let to = s.top_joint.as_ref().map_or(route.socket, |j| j.pos);
            if !self.way_clear(from, to, s.diameter * 0.5 * CLEAR_SHARE) {
                return Err(EditError::CannotAttach(AddNote::HitsModel));
            }
            from = to;
        }
        Ok(())
    }

    /// Whether nothing of the models comes within `radius` of the axis from
    /// `from` to `to`. The way starts and ends outside the models, so one
    /// through a model crosses its surface and is caught there.
    fn way_clear(&self, from: Vec3, to: Vec3, radius: f64) -> bool {
        let steps = (from.distance_to(to) / CLEAR_STEP_MM).ceil().max(1.0) as usize;
        let bvhs: Vec<_> = (0..self.models.len()).map(|i| self.bvh(i)).collect();
        (0..=steps).all(|k| {
            let p = from.lerp(to, k as f64 / steps as f64);
            bvhs.iter().all(|bvh| {
                bvh.closest_point_to_point(p, 0.0, radius)
                    .is_none_or(|hit| hit.distance >= radius)
            })
        })
    }

    /// The preview of a part edit of support `id`: `moved` is the session
    /// with the edit made but not checked, or why it cannot be.
    fn preview_part_edit(
        &self,
        id: &str,
        moved: Result<Self, EditError>,
    ) -> Result<(EditPreview, Option<Self>), EditError> {
        let taken = self.drawing_of(&[id.to_owned()])?;
        let refused = |shown: &Self, err: EditError| -> Result<_, EditError> {
            let note = match err {
                EditError::CannotAttach(note) => Some(note),
                EditError::NothingThere => return Err(EditError::NothingThere),
                _ => None,
            };
            Ok((
                EditPreview {
                    mesh: shown.drawing_of(&[id.to_owned()])?,
                    valid: false,
                    attached: false,
                    note,
                    taken: None,
                    brace: None,
                },
                None,
            ))
        };
        let moved = match moved {
            Ok(moved) => moved,
            Err(err) => return refused(self, err),
        };
        match moved.clone().checked(id) {
            Ok(done) => {
                let mesh = done.drawing_of(&[id.to_owned()])?;
                Ok((
                    EditPreview {
                        mesh,
                        valid: true,
                        attached: false,
                        note: None,
                        taken: Some(taken),
                        brace: None,
                    },
                    Some(done),
                ))
            }
            Err(err) => refused(&moved, err),
        }
    }
}

#[cfg(test)]
mod part_edit_tests {
    use super::*;
    use crate::session::session_tests::bare_slab;

    /// A slab 12 mm up with one support at each of two points, the second
    /// hung from the first.
    fn two_supports() -> (SupportSession, String, String) {
        let mut session = bare_slab(12.0, crate::SupportRequest::default());
        session
            .add_at([5.0, 5.0, 12.0], &AddOptions::default())
            .unwrap();
        let hung = AddOptions {
            attach: true,
            ..AddOptions::default()
        };
        session.add_at([7.0, 5.0, 12.0], &hung).unwrap();
        let first = session.tip_near([5.0, 5.0, 12.0], 0.5).unwrap();
        let second = session.tip_near([7.0, 5.0, 12.0], 0.5).unwrap();
        (session, first, second)
    }

    /// #377: a trunk moved across the plate takes its foot and elbows
    /// along, keeps its tip, leans into it no more than a hand support may,
    /// and the support hung from it is routed again; one under the model
    /// is refused and changes nothing.
    #[test]
    fn a_trunk_moves_across_the_plate_and_what_hangs_follows() {
        let (mut session, first, _) = two_supports();
        let before = session.parts_of(&first).unwrap();
        session.move_trunk(&first, [-4.0, 0.0]).unwrap();
        let after = session.parts_of(&first).expect("the same support");
        assert_eq!(after.tip, before.tip, "its tip stays");
        let foot = after.foot.unwrap();
        assert!((foot[0] - (before.foot.unwrap()[0] - 4.0)).abs() < 1e-9);
        let mut way = vec![after.anchor];
        way.extend(after.elbows.iter().map(|(_, p)| *p));
        way.push(after.socket);
        for pair in way.windows(2) {
            assert!(lean_deg(v(pair[0]), v(pair[1])) <= MANUAL_MAX_ANGLE_DEG + 1e-6);
        }
        assert!(
            session.tip_near([7.0, 5.0, 12.0], 0.5).is_some(),
            "the hung support is still there"
        );
        let state = session.state.clone();
        // Under the slab's middle, the way up runs through it.
        assert!(session.move_trunk(&first, [20.0, 0.0]).is_err());
        assert_eq!(session.state, state, "a refusal leaves it as it was");
    }

    /// #377: an elbow moved out of order or through the model is refused; one
    /// moved aside takes the shaft with it.
    #[test]
    fn an_elbow_moves_and_the_shaft_follows() {
        let (mut session, first, _) = two_supports();
        let parts = session.parts_of(&first).unwrap();
        let (elbow, at) = parts.elbows[0].clone();
        let below = [at[0], at[1], parts.anchor[2] - 1.0];
        assert_eq!(
            session.move_elbow(&first, &elbow, below),
            Err(EditError::CannotAttach(AddNote::OutOfOrder))
        );
        let aside = [at[0] - 1.5, at[1], at[2]];
        session.move_elbow(&first, &elbow, aside).unwrap();
        let moved = session.parts_of(&first).unwrap();
        let now = moved.elbows.iter().find(|(e, _)| *e == elbow).unwrap().1;
        assert!((now[0] - aside[0]).abs() < 1e-9);
        assert_eq!(moved.tip, parts.tip);
        let (preview, _) = session
            .preview_move_elbow(&first, &elbow, [at[0], at[1], 13.0])
            .unwrap();
        assert!(!preview.valid, "above the tip's socket is out of order");
    }

    /// #377: a support's tip, shaft and foot take their own widths in place,
    /// its route as it is, even one others hang from; a tip wider than the
    /// shaft is refused.
    #[test]
    fn parts_take_their_own_widths_in_place() {
        let (mut session, first, _) = two_supports();
        let before = session.parts_of(&first).unwrap();
        session
            .resize_parts(
                &first,
                PartSizes {
                    tip_mm: Some(0.5),
                    trunk_mm: Some(1.4),
                    foot_mm: Some(3.0),
                },
            )
            .unwrap();
        let after = session.parts_of(&first).unwrap();
        assert_eq!(after.tip_mm, 0.5);
        assert_eq!(after.trunk_mm, Some(1.4));
        assert_eq!(after.foot_mm, Some(3.0));
        assert_eq!(after.elbows, before.elbows, "the route stays");
        let state = session.state.clone();
        assert_eq!(
            session.resize_parts(
                &first,
                PartSizes {
                    tip_mm: Some(2.0),
                    ..PartSizes::default()
                },
            ),
            Err(EditError::CannotAttach(AddNote::TipOverShaft))
        );
        assert_eq!(session.state, state);
        assert!(session.mesh().unwrap().is_some(), "it still draws");
    }

    /// #261: an imported support is placed with its numbers on the route it
    /// took; a route through the model is not taken, the support is placed
    /// as Add places it.
    #[test]
    fn an_imported_support_keeps_its_route_where_it_may() {
        let mut session = bare_slab(12.0, crate::SupportRequest::default());
        let mut shape = crate::zone_presets::support_sizes()[0].clone();
        shape.tip_diameter_mm = 0.35;
        shape.trunk_diameter_mm = 0.9;
        let plan = ImportedPlan {
            at: [5.0, 5.0, 12.0],
            shape: Some(shape),
            route: Some(([2.0, 5.0], vec![[2.0, 5.0, 6.0]])),
        };
        assert_eq!(session.add_imported(&plan), Ok(true));
        let id = session.tip_near([5.0, 5.0, 12.0], 0.5).unwrap();
        let parts = session.parts_of(&id).unwrap();
        assert!((parts.foot.unwrap()[0] - 2.0).abs() < 1e-9, "{parts:?}");
        assert!(
            parts.elbows.iter().any(|(_, p)| (p[2] - 6.0).abs() < 1e-9),
            "{parts:?}"
        );
        assert!((parts.tip_mm - 0.35).abs() < 1e-6, "{parts:?}");
        assert_eq!(parts.trunk_mm, Some(0.9));
        // An elbow over the tip's socket is no elbow of a route up to it.
        let through = ImportedPlan {
            at: [30.0, 5.0, 12.0],
            shape: None,
            route: Some(([30.0, 5.0], vec![[30.0, 5.0, 13.5]])),
        };
        assert_eq!(
            session.add_imported(&through),
            Ok(true),
            "out of order goes"
        );
        let wrong = ImportedPlan {
            at: [40.0, 5.0, 12.0],
            shape: None,
            // A foot 25 mm aside of a tip 9 mm up: too far over to print.
            route: Some(([40.0, 30.0], vec![[40.0, 30.0, 3.0]])),
        };
        assert_eq!(session.add_imported(&wrong), Ok(false));
        assert!(session.tip_near([40.0, 5.0, 12.0], 0.5).is_some());
    }

    /// #261: an imported tip too short for a route here takes the shipped
    /// length, and the support is still made.
    #[test]
    fn a_short_imported_tip_takes_a_length_a_route_needs() {
        let mut session = bare_slab(5.0, crate::SupportRequest::default());
        let mut shape = crate::zone_presets::support_sizes()[0].clone();
        shape.tip_length_mm = 0.8;
        let plan = ImportedPlan {
            at: [5.0, 5.0, 5.0],
            shape: Some(shape),
            route: Some(([5.0, 5.0], Vec::new())),
        };
        assert_eq!(session.add_imported(&plan), Ok(true));
        assert!(session.tip_near([5.0, 5.0, 5.0], 0.5).is_some());
    }

    /// A run's own supports pass the check a moved shaft goes through.
    #[test]
    fn a_run_s_shafts_pass_the_model_check() {
        let session = crate::start_supports(
            &crate::session::session_tests::slab_scene(9.0),
            crate::SupportRequest::default(),
        )
        .unwrap()
        .join()
        .unwrap()
        .sets
        .remove(0)
        .session;
        let ids: Vec<String> = session.state.collections.trunks.keys().cloned().collect();
        assert!(!ids.is_empty());
        for id in ids {
            assert!(session.shaft_clear(&id).is_ok(), "{id}");
        }
    }
}
