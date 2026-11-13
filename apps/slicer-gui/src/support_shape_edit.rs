/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the Supports tab's shape panel (#564) edits in a zone preset: the
//! sizes it shows, the number each field reads and writes, and the rows of
//! where each size goes.
//!
//! The tip is a size's own. The rest of the support (trunk, tip length,
//! foot, insertion, break point) is shared: a change goes to every size
//! that had the same value, so the four sizes a preset places by hand keep
//! one structure (a support is the same support with another tip) while a
//! size made thicker on purpose, as the Statues' overhang one, keeps its
//! own.

use sc_geom::df_supports::zone_presets::ZonePreset;
use sc_geom::df_supports::{BaseTip, SupportSize};

/// The sizes a hand click picks with keys 1 to 4, smallest first: always
/// shown, so the sizes read the same in every preset.
const HAND_SIZES: [&str; 4] = ["extrasmall", "small", "medium", "large"];

/// The short name of size `id` on its chip.
pub(crate) fn short_name(id: &str) -> String {
    match id {
        "extrasmall" => "XS".to_owned(),
        "small" => "S".to_owned(),
        "medium" => "M".to_owned(),
        "large" => "L".to_owned(),
        "light" => "LT".to_owned(),
        "factory-medium" => "XL".to_owned(),
        "heavy" => "XXL".to_owned(),
        other => other.chars().take(3).collect::<String>().to_uppercase(),
    }
}

/// The sizes the panel shows, as indices into the preset's sizes: the four
/// hand sizes in their order (XS S M L), then every other size a zone of
/// the preset places, in the preset's order. Never by tip: a size whose tip
/// is made bigger than the next one keeps its place, and so does its chip.
pub(crate) fn shown_sizes(zones: &ZonePreset) -> Vec<usize> {
    let r = &zones.rules;
    let used: Vec<&str> = r
        .first_layer
        .iter()
        .filter(|b| b.enabled)
        .map(|b| b.size.as_str())
        .chain(
            r.reinforcement
                .iter()
                .filter(|b| b.enabled)
                .map(|b| b.size.as_str()),
        )
        .chain(r.overhang.enabled.then_some(r.overhang.size.as_str()))
        .chain(
            r.stabilization
                .enabled
                .then_some(r.stabilization.size.as_str()),
        )
        .collect();
    let mut shown: Vec<usize> = r
        .sizes
        .iter()
        .enumerate()
        .filter(|(_, s)| HAND_SIZES.contains(&s.id.as_str()) || used.contains(&s.id.as_str()))
        .map(|(i, _)| i)
        .collect();
    shown.sort_by_key(|&i| {
        HAND_SIZES
            .iter()
            .position(|&id| id == r.sizes[i].id)
            .unwrap_or(HAND_SIZES.len() + i)
    });
    shown
}

/// Fields of the panel, by code: the tip is the size's own, the next eight
/// are shared with the sizes of the same value, the last two the bracing's.
pub(crate) const FIELDS: usize = 11;

/// The number of field `code` for the size at `size`.
fn read(zones: &ZonePreset, size: &SupportSize, code: usize) -> f64 {
    match code {
        0 => size.tip_diameter_mm,
        1 => size.trunk_diameter_mm,
        2 => size.tip_length_mm,
        3 => size.base_diameter_mm,
        4 => size.penetration_mm,
        5 => size.break_point_mm,
        6 => size.base_thickness_mm,
        7 => size.join_cone_mm,
        8 => size.base_tip.map_or(size.tip_length_mm, |t| t.length_mm),
        9 => zones.bracing.diameter_mm,
        10 => zones.bracing.start_mm,
        _ => 0.0,
    }
}

/// Every field of the size at `index`, in code order.
pub(crate) fn values(zones: &ZonePreset, index: usize) -> [f64; FIELDS] {
    let mut out = [0.0; FIELDS];
    if let Some(size) = zones.rules.sizes.get(index) {
        for (code, v) in out.iter_mut().enumerate() {
            *v = read(zones, size, code);
        }
    }
    out
}

fn write(size: &mut SupportSize, code: usize, v: f64) {
    match code {
        0 => {
            // A tip on the model that had the top's width keeps it.
            if let Some(t) = size.base_tip.as_mut()
                && (t.diameter_mm - size.tip_diameter_mm).abs() < 1e-9
            {
                t.diameter_mm = v;
            }
            size.tip_diameter_mm = v;
        }
        1 => size.trunk_diameter_mm = v,
        2 => size.tip_length_mm = v,
        3 => size.base_diameter_mm = v,
        4 => size.penetration_mm = v,
        5 => size.break_point_mm = v,
        6 => size.base_thickness_mm = v,
        7 => size.join_cone_mm = v,
        8 => {
            let tip = size.tip_diameter_mm;
            size.base_tip = Some(BaseTip {
                length_mm: v,
                diameter_mm: size.base_tip.map_or(tip, |t| t.diameter_mm),
            });
        }
        _ => {}
    }
}

/// What a field goes back to: the number the built-in preset `base` has
/// for the same size (by id), or for the whole preset (the bracing's two).
/// `None` where `base` has no such size.
pub(crate) fn defaults(
    base: &ZonePreset,
    zones: &ZonePreset,
    index: usize,
) -> [Option<f64>; FIELDS] {
    let mut out = [None; FIELDS];
    let own = zones
        .rules
        .sizes
        .get(index)
        .and_then(|s| base.rules.sizes.iter().position(|b| b.id == s.id));
    for (code, slot) in out.iter_mut().enumerate() {
        *slot = match (code, own) {
            (9 | 10, _) => Some(values(base, 0)[code]),
            (_, Some(j)) => Some(values(base, j)[code]),
            (_, None) => None,
        };
    }
    out
}

/// Which of `bases` the preset `zones` was most likely made from: the one
/// it shares the most numbers with (fields of the sizes with the same id,
/// and the zones' sizes and spacings). Asked when that was not kept, as
/// after the app opens again with a preset changed the time before.
pub(crate) fn closest_base(bases: &[&ZonePreset], zones: &ZonePreset) -> Option<usize> {
    let same = |a: f64, b: f64| (a - b).abs() < 1e-9;
    let score = |base: &ZonePreset| -> usize {
        let mut n = 0;
        for (i, size) in zones.rules.sizes.iter().enumerate() {
            if let Some(j) = base.rules.sizes.iter().position(|b| b.id == size.id) {
                let (a, b) = (values(zones, i), values(base, j));
                n += 1 + (0..FIELDS).filter(|&k| same(a[k], b[k])).count();
            }
        }
        for zone in zones_of(zones) {
            if let (Some((a, x)), Some((b, y))) = (
                zone_size_and_spacing(zones, zone),
                zone_size_and_spacing(base, zone),
            ) {
                n += usize::from(a == b) + usize::from(same(x, y));
            }
        }
        n + usize::from(same(
            zones.rules.overhang.angle_deg,
            base.rules.overhang.angle_deg,
        ))
    };
    bases
        .iter()
        .enumerate()
        .max_by_key(|&(i, b)| (score(b), std::cmp::Reverse(i)))
        .map(|(i, _)| i)
}

/// Sets field `code` to `v` on the size at `index` alone (each size keeps
/// its own numbers); the bracing's two for the whole preset. Values go
/// through the engine's bounds.
pub(crate) fn set_value(zones: &mut ZonePreset, index: usize, code: usize, v: f64) {
    match code {
        9 => zones.bracing.diameter_mm = v.clamp(0.1, 10.0),
        10 => zones.bracing.start_mm = v.clamp(0.0, 100.0),
        _ => {
            if let Some(size) = zones.rules.sizes.get_mut(index) {
                write(size, code, v);
                *size = size.clamped();
            }
        }
    }
}

/// A zone of the preset, as a row of the panel lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Zone {
    Overhang,
    Stabilization,
    /// The island band at this index of `first_layer`.
    Island(usize),
    /// The island body band at this index of `reinforcement`.
    IslandBody(usize),
}

/// The zones of the preset that place supports, in the panel's order.
pub(crate) fn zones_of(zones: &ZonePreset) -> Vec<Zone> {
    let r = &zones.rules;
    let mut out = Vec::new();
    if r.overhang.enabled {
        out.push(Zone::Overhang);
    }
    if r.stabilization.enabled {
        out.push(Zone::Stabilization);
    }
    out.extend(
        r.first_layer
            .iter()
            .enumerate()
            .filter(|(_, b)| b.enabled)
            .map(|(i, _)| Zone::Island(i)),
    );
    out.extend(
        r.reinforcement
            .iter()
            .enumerate()
            .filter(|(_, b)| b.enabled)
            .map(|(i, _)| Zone::IslandBody(i)),
    );
    out
}

/// The size a zone places, and how far apart.
pub(crate) fn zone_size_and_spacing(zones: &ZonePreset, zone: Zone) -> Option<(&str, f64)> {
    let r = &zones.rules;
    match zone {
        Zone::Overhang => Some((&r.overhang.size, r.overhang.spacing_mm)),
        Zone::Stabilization => Some((&r.stabilization.size, r.stabilization.spacing_mm)),
        Zone::Island(i) => r
            .first_layer
            .get(i)
            .map(|b| (b.size.as_str(), b.spacing_mm)),
        Zone::IslandBody(i) => r
            .reinforcement
            .get(i)
            .map(|b| (b.size.as_str(), b.spacing_mm)),
    }
}

/// Makes `zone` place the size `id`.
pub(crate) fn set_zone_size(zones: &mut ZonePreset, zone: Zone, id: &str) {
    let r = &mut zones.rules;
    let slot = match zone {
        Zone::Overhang => Some(&mut r.overhang.size),
        Zone::Stabilization => Some(&mut r.stabilization.size),
        Zone::Island(i) => r.first_layer.get_mut(i).map(|b| &mut b.size),
        Zone::IslandBody(i) => r.reinforcement.get_mut(i).map(|b| &mut b.size),
    };
    if let Some(slot) = slot {
        id.clone_into(slot);
    }
}

/// Spaces `zone`'s supports `mm` apart, inside the engine's bounds.
pub(crate) fn set_zone_spacing(zones: &mut ZonePreset, zone: Zone, mm: f64) {
    let mm = mm.clamp(0.2, 50.0);
    let r = &mut zones.rules;
    match zone {
        Zone::Overhang => r.overhang.spacing_mm = mm,
        Zone::Stabilization => r.stabilization.spacing_mm = mm,
        Zone::Island(i) => {
            if let Some(b) = r.first_layer.get_mut(i) {
                b.spacing_mm = mm;
            }
        }
        Zone::IslandBody(i) => {
            if let Some(b) = r.reinforcement.get_mut(i) {
                b.spacing_mm = mm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_geom::df_supports::zone_presets::{figures_150, minis_30};

    fn index(zones: &ZonePreset, id: &str) -> usize {
        zones
            .rules
            .sizes
            .iter()
            .position(|s| s.id == id)
            .expect("the size ships")
    }

    /// Miniatures shows its four sizes; Statues also the thick overhang one
    /// it places, after them.
    #[test]
    fn the_sizes_shown_are_the_hand_ones_and_those_a_zone_places() {
        let names = |z: &ZonePreset| -> Vec<String> {
            shown_sizes(z)
                .iter()
                .map(|&i| short_name(&z.rules.sizes[i].id))
                .collect()
        };
        assert_eq!(names(&minis_30()), ["XS", "S", "M", "L"]);
        assert_eq!(names(&figures_150()), ["XS", "S", "M", "L", "XL"]);
    }

    /// Every number changes its own size alone: a tip, a trunk, a foot.
    #[test]
    fn each_size_keeps_its_own_numbers() {
        let mut z = figures_150();
        let small = index(&z, "small");
        let before = z.clone();
        for (code, v) in [(0, 0.27), (1, 1.25), (2, 3.0), (3, 6.0), (6, 0.4)] {
            set_value(&mut z, small, code, v);
        }
        assert_eq!(
            z.rules.sizes[small].base_tip.map(|t| t.diameter_mm),
            Some(0.27)
        );
        let changed: Vec<usize> = (0..z.rules.sizes.len())
            .filter(|&i| z.rules.sizes[i] != before.rules.sizes[i])
            .collect();
        assert_eq!(changed, [small]);
        let medium = index(&z, "medium");
        assert_eq!(values(&z, medium), values(&before, medium));
    }

    /// A field goes back to the built-in preset's number for its size; a
    /// size the built-in preset lacks has nothing to go back to.
    #[test]
    fn a_field_goes_back_to_the_built_in_number_of_its_size() {
        let base = minis_30();
        let mut z = base.clone();
        let medium = index(&z, "medium");
        set_value(&mut z, medium, 0, 0.5);
        set_value(&mut z, medium, 1, 2.0);
        let d = defaults(&base, &z, medium);
        assert_eq!(d[0], Some(0.30));
        assert_eq!(d[1], Some(base.rules.sizes[medium].trunk_diameter_mm));
        assert_eq!(d[9], Some(base.bracing.diameter_mm));
        z.rules.sizes[medium].id = "mine".to_owned();
        let d = defaults(&base, &z, medium);
        assert_eq!(d[0], None);
        assert_eq!(d[9], Some(base.bracing.diameter_mm));
    }

    /// A tip may grow past the next size's: each size keeps its place, so
    /// the chip picked stays the size being edited.
    #[test]
    fn a_tip_grown_past_the_next_size_keeps_every_size_in_its_place() {
        let mut z = minis_30();
        let small = index(&z, "small");
        let before = shown_sizes(&z);
        set_value(&mut z, small, 0, 0.45);
        assert_eq!(shown_sizes(&z), before);
        let extrasmall = index(&z, "extrasmall");
        set_value(&mut z, extrasmall, 0, 0.5);
        assert_eq!(shown_sizes(&z), before);
        assert_eq!(z.rules.sizes[extrasmall].tip_diameter_mm, 0.5);
        assert_eq!(z.rules.sizes[small].tip_diameter_mm, 0.45);
        assert_eq!(z.rules.sizes[index(&z, "medium")].tip_diameter_mm, 0.30);
    }

    /// A preset changed by hand is told apart as made from the built-in
    /// one it shares the most with.
    #[test]
    fn a_changed_preset_is_matched_to_the_built_in_one_it_came_from() {
        let bases = [minis_30(), figures_150()];
        let refs: Vec<&ZonePreset> = bases.iter().collect();
        for (i, base) in bases.iter().enumerate() {
            let mut z = base.clone();
            let small = index(&z, "small");
            for (code, v) in [(0, 1.12), (1, 2.86), (2, 5.49), (3, 2.52)] {
                set_value(&mut z, small, code, v);
            }
            assert_eq!(closest_base(&refs, &z), Some(i));
        }
    }

    /// Values go through the engine's bounds, and the bracing is the
    /// preset's.
    #[test]
    fn values_stay_inside_the_engine_s_bounds() {
        let mut z = minis_30();
        let small = index(&z, "small");
        set_value(&mut z, small, 0, 0.0);
        assert_eq!(z.rules.sizes[small].tip_diameter_mm, 0.05);
        set_value(&mut z, small, 9, 1.5);
        assert_eq!(values(&z, small)[9], 1.5);
        assert_eq!(z.bracing.diameter_mm, 1.5);
    }

    /// Every zone that places supports is a row, and a row's size and
    /// spacing are the zone's.
    #[test]
    fn the_zones_read_and_write_their_size_and_spacing() {
        let mut z = minis_30();
        let rows = zones_of(&z);
        assert_eq!(rows[0], Zone::Overhang);
        assert_eq!(rows[1], Zone::Stabilization);
        assert_eq!(rows.len(), 2 + 3 + 3);
        assert_eq!(
            zone_size_and_spacing(&z, Zone::Overhang),
            Some(("small", 1.8))
        );
        set_zone_size(&mut z, Zone::Overhang, "medium");
        set_zone_spacing(&mut z, Zone::Overhang, 2.2);
        assert_eq!(
            zone_size_and_spacing(&z, Zone::Overhang),
            Some(("medium", 2.2))
        );
        set_zone_spacing(&mut z, Zone::Island(0), 0.0);
        assert_eq!(
            zone_size_and_spacing(&z, Zone::Island(0)).map(|s| s.1),
            Some(0.2)
        );
    }
}
