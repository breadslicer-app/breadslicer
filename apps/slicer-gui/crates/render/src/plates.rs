/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The build plates of a scene (#545): one or more of the printer's plate,
//! laid out in rows of [`PLATES_PER_ROW`], the first where the one plate
//! always stood (centred on the origin), the others to its right and, row
//! after row, behind it. A point belongs to the plate whose footprint holds
//! it.

use glam::Vec3;

use crate::build_volume::BuildVolume;

/// How many plates a row holds before the next row starts behind it.
pub const PLATES_PER_ROW: u32 = 4;

/// The room between two plates, mm: more than the rulers and the front
/// label take around a plate, so neither reaches the next one.
pub const PLATE_GAP_MM: f32 = 40.0;

/// The most plates a scene lays out.
pub const MAX_PLATES: u32 = 32;

/// A plate's name being edited where it lies (#545): the plate, and the
/// bytes of its name the caret and the selection's other end stand at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NameEdit {
    pub plate: usize,
    pub caret: usize,
    pub anchor: usize,
}

/// The plates of a scene and which is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plates {
    count: u32,
    active: u32,
}

impl Default for Plates {
    fn default() -> Self {
        Self::one()
    }
}

impl Plates {
    /// The one plate every scene has.
    pub const fn one() -> Self {
        Self {
            count: 1,
            active: 0,
        }
    }

    /// `count` plates (at least one, at most [`MAX_PLATES`]) with `active`
    /// the active one (the last when past it).
    pub fn new(count: u32, active: u32) -> Self {
        let count = count.clamp(1, MAX_PLATES);
        Self {
            count,
            active: active.min(count - 1),
        }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn active(&self) -> u32 {
        self.active
    }

    /// The same plates with `active` the active one.
    pub fn with_active(self, active: u32) -> Self {
        Self::new(self.count, active)
    }

    /// The same plates with `count` of them.
    pub fn with_count(self, count: u32) -> Self {
        Self::new(count, self.active)
    }
}

/// The tools of the active plate (#545) lie on its rim, right of its back
/// right corner, one under the other: Add a plate, Arrange this plate,
/// Remove this plate. Each is a square this wide, mm.
pub const TOOL_MM: f32 = 14.0;
/// The room between two tools, mm.
pub const TOOL_GAP_MM: f32 = 2.5;
/// How far right of the plate's edge the tools stand, mm.
pub const TOOL_INSET_MM: f32 = 5.0;
/// How many tools.
pub const TOOL_COUNT: u32 = 3;

/// The tool of a plate of `volume` at `x, y` of the plate's own frame
/// (its centre the origin): 0 add, 1 arrange, 2 remove.
pub fn tool_at(volume: &BuildVolume, x: f32, y: f32) -> Option<u32> {
    let left = volume.width_mm() / 2.0 + TOOL_INSET_MM;
    if x < left || x > left + TOOL_MM {
        return None;
    }
    let down = volume.depth_mm() / 2.0 - y;
    if down < 0.0 {
        return None;
    }
    let i = (down / (TOOL_MM + TOOL_GAP_MM)).floor();
    let within = down - i * (TOOL_MM + TOOL_GAP_MM);
    (i < TOOL_COUNT as f32 && within <= TOOL_MM).then_some(i as u32)
}

/// A plate's name (#545) lies on its rim behind its back edge, from its
/// back left corner: a strip this tall, mm.
pub const NAME_H_MM: f32 = 7.0;
/// How far behind the plate's back edge the strip starts, mm.
pub const NAME_GAP_MM: f32 = 4.0;
/// The longest the strip runs along the back edge, mm.
pub const NAME_MAX_W_MM: f32 = 70.0;

/// The strip a plate of `volume` shows its name in, in the plate's own
/// frame: `[min_x, min_y, max_x, max_y]`.
pub fn name_box(volume: &BuildVolume) -> [f32; 4] {
    let x0 = -volume.width_mm() / 2.0;
    let y0 = volume.depth_mm() / 2.0 + NAME_GAP_MM;
    let w = NAME_MAX_W_MM.min(volume.width_mm() * 0.6);
    [x0, y0, x0 + w, y0 + NAME_H_MM]
}

/// Whether `x, y` of a plate's own frame is on its name.
pub fn on_name(volume: &BuildVolume, x: f32, y: f32) -> bool {
    let [x0, y0, x1, y1] = name_box(volume);
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// How far apart the plates of `volume` stand, from one's centre to the
/// next one's, along X and along Y.
pub fn pitch(volume: &BuildVolume) -> (f32, f32) {
    (
        volume.width_mm() + PLATE_GAP_MM,
        volume.depth_mm() + PLATE_GAP_MM,
    )
}

/// The centre of plate `index` of `volume`, on the floor: the first is the
/// origin.
pub fn offset(volume: &BuildVolume, index: u32) -> Vec3 {
    let (px, py) = pitch(volume);
    let col = index % PLATES_PER_ROW;
    let row = index / PLATES_PER_ROW;
    Vec3::new(col as f32 * px, row as f32 * py, 0.0)
}

/// How far the last column and the last row of `plates` stand from the
/// first plate: what the plates take past its corner.
pub fn extent(volume: &BuildVolume, plates: Plates) -> Vec3 {
    let columns = plates.count().min(PLATES_PER_ROW);
    let (px, _) = pitch(volume);
    let last = offset(volume, plates.count() - 1);
    Vec3::new((columns - 1) as f32 * px, last.y, 0.0)
}

/// The plate of `plates` whose cell holds `x, y`: the plate's footprint and
/// half the room around it. `None` past every plate.
pub fn cell_at(volume: &BuildVolume, plates: Plates, x: f32, y: f32) -> Option<u32> {
    let (px, py) = pitch(volume);
    let col = (x / px).round();
    let row = (y / py).round();
    if col < 0.0 || row < 0.0 || col >= PLATES_PER_ROW as f32 {
        return None;
    }
    let index = row as u32 * PLATES_PER_ROW + col as u32;
    (index < plates.count()).then_some(index)
}

/// The plate of `plates` whose footprint holds `x, y`. `None` off every
/// plate, between two of them as well.
pub fn plate_at(volume: &BuildVolume, plates: Plates, x: f32, y: f32) -> Option<u32> {
    let index = cell_at(volume, plates, x, y)?;
    let centre = offset(volume, index);
    let (hw, hd) = (volume.width_mm() / 2.0, volume.depth_mm() / 2.0);
    ((x - centre.x).abs() <= hw && (y - centre.y).abs() <= hd).then_some(index)
}

/// The plate a model whose footprint is centred at `x, y` belongs to: the
/// plate of the cell it is in, the first when it is past every plate.
pub fn plate_of(volume: &BuildVolume, plates: Plates, x: f32, y: f32) -> u32 {
    cell_at(volume, plates, x, y).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume() -> BuildVolume {
        BuildVolume::new(200.0, 100.0, 150.0)
    }

    #[test]
    fn plates_go_in_rows_of_four_from_the_origin() {
        let v = volume();
        assert_eq!(offset(&v, 0), Vec3::ZERO);
        assert_eq!(offset(&v, 1), Vec3::new(240.0, 0.0, 0.0));
        assert_eq!(offset(&v, 3), Vec3::new(720.0, 0.0, 0.0));
        assert_eq!(offset(&v, 4), Vec3::new(0.0, 140.0, 0.0));
        assert_eq!(offset(&v, 9), Vec3::new(240.0, 280.0, 0.0));
    }

    #[test]
    fn a_point_is_on_the_plate_under_it() {
        let v = volume();
        let plates = Plates::new(6, 0);
        assert_eq!(plate_at(&v, plates, 10.0, -40.0), Some(0));
        assert_eq!(plate_at(&v, plates, 250.0, 10.0), Some(1));
        assert_eq!(plate_at(&v, plates, 40.0, 150.0), Some(4));
        // Between plates 0 and 1, and where plate 7 would be.
        assert_eq!(plate_at(&v, plates, 120.0, 0.0), None);
        assert_eq!(plate_at(&v, plates, 720.0, 140.0), None);
        // A model between two plates belongs to the nearer.
        assert_eq!(plate_of(&v, plates, 130.0, 0.0), 1);
        assert_eq!(plate_of(&v, plates, -500.0, 0.0), 0);
    }

    #[test]
    fn the_plates_reach_to_their_last_column_and_row() {
        let v = volume();
        assert_eq!(extent(&v, Plates::one()), Vec3::ZERO);
        assert_eq!(extent(&v, Plates::new(3, 0)), Vec3::new(480.0, 0.0, 0.0));
        assert_eq!(extent(&v, Plates::new(6, 0)), Vec3::new(720.0, 140.0, 0.0));
    }

    #[test]
    fn the_tools_stand_right_of_the_back_right_corner() {
        let v = volume();
        // The plate is 200 x 100: its back right corner is at 100, 50.
        assert_eq!(tool_at(&v, 112.0, 43.0), Some(0));
        assert_eq!(tool_at(&v, 112.0, 43.0 - 16.5), Some(1));
        assert_eq!(tool_at(&v, 112.0, 43.0 - 33.0), Some(2));
        assert_eq!(tool_at(&v, 112.0, 43.0 - 49.5), None, "past the last");
        assert_eq!(tool_at(&v, 112.0, 51.0), None, "behind the plate");
        assert_eq!(tool_at(&v, 102.0, 43.0), None, "on the plate's edge");
        assert_eq!(tool_at(&v, 112.0, 35.75), None, "between two");
    }

    #[test]
    fn the_name_lies_behind_the_back_left_corner() {
        let v = volume();
        // The plate is 200 x 100: its back left corner is at -100, 50.
        assert!(on_name(&v, -95.0, 57.0));
        assert!(!on_name(&v, -95.0, 52.0), "between the edge and the name");
        assert!(!on_name(&v, -20.0, 57.0), "past its end");
    }

    #[test]
    fn the_active_plate_stays_among_them() {
        assert_eq!(Plates::new(3, 7).active(), 2);
        assert_eq!(Plates::new(0, 0).count(), 1);
        assert_eq!(Plates::new(5, 4).with_count(2).active(), 1);
    }
}
