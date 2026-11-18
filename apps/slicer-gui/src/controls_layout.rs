/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Control layouts (#609): the mouse of the 3D view as other 3D programs
//! have it, so someone coming from one of them does not learn the basics
//! again. A layout says, for each button held with each modifier, whether a
//! drag orbits, pans or zooms; the left button alone always selects and
//! moves models, and a right click without a drag opens the menu.

use serde::{Deserialize, Serialize};

use crate::NavMap;

/// What a drag of a button does to the camera, as the view reads it.
const NONE: i32 = 0;
const ORBIT: i32 = 1;
const PAN: i32 = 2;
const ZOOM: i32 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ControlsLayout {
    /// Breadslicer's own, Blender's: middle orbits, Shift+middle pans,
    /// Ctrl+middle zooms; the right button as the middle one.
    #[default]
    Breadslicer,
    /// As most resin slicers: right orbits, middle pans.
    ClassicSlicer,
    /// Middle pans, Shift+middle orbits.
    Fusion,
    /// Middle orbits, Ctrl+middle pans, Shift+middle zooms.
    SolidWorks,
    /// Alt with the left, middle or right button: orbit, pan, zoom.
    Maya,
}

impl ControlsLayout {
    /// Every layout, in the order of the Preferences combo.
    pub(crate) const ALL: [ControlsLayout; 5] = [
        ControlsLayout::Breadslicer,
        ControlsLayout::ClassicSlicer,
        ControlsLayout::Fusion,
        ControlsLayout::SolidWorks,
        ControlsLayout::Maya,
    ];

    pub(crate) fn combo_index(self) -> i32 {
        Self::ALL.iter().position(|&l| l == self).unwrap_or(0) as i32
    }

    pub(crate) fn from_combo_index(index: i32) -> Self {
        usize::try_from(index)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
            .unwrap_or_default()
    }

    /// The layout's name, as the combo shows it. Other programs keep their
    /// own names: these only say whose mouse it copies.
    pub(crate) fn label(self) -> String {
        match self {
            ControlsLayout::Breadslicer => {
                crate::i18n::ctr!("Controls", "Breadslicer (as Blender)").to_string()
            }
            ControlsLayout::ClassicSlicer => {
                crate::i18n::ctr!("Controls", "Classic resin slicer").to_string()
            }
            ControlsLayout::Fusion => "Fusion 360".to_owned(),
            ControlsLayout::SolidWorks => "SolidWorks".to_owned(),
            ControlsLayout::Maya => "Maya".to_owned(),
        }
    }

    /// What each button and modifier does to the camera.
    pub(crate) fn nav_map(self) -> NavMap {
        let none = NavMap {
            middle: NONE,
            middle_shift: NONE,
            middle_ctrl: NONE,
            middle_alt: NONE,
            right: NONE,
            right_shift: NONE,
            right_ctrl: NONE,
            right_alt: NONE,
            left_alt: NONE,
        };
        match self {
            ControlsLayout::Breadslicer => NavMap {
                middle: ORBIT,
                middle_shift: PAN,
                middle_ctrl: ZOOM,
                right: ORBIT,
                right_shift: PAN,
                ..none
            },
            ControlsLayout::ClassicSlicer => NavMap {
                middle: PAN,
                middle_shift: ORBIT,
                middle_ctrl: ZOOM,
                right: ORBIT,
                right_shift: PAN,
                ..none
            },
            ControlsLayout::Fusion => NavMap {
                middle: PAN,
                middle_shift: ORBIT,
                ..none
            },
            ControlsLayout::SolidWorks => NavMap {
                middle: ORBIT,
                middle_ctrl: PAN,
                middle_shift: ZOOM,
                ..none
            },
            ControlsLayout::Maya => NavMap {
                left_alt: ORBIT,
                middle_alt: PAN,
                right_alt: ZOOM,
                middle: PAN,
                ..none
            },
        }
    }

    /// One line per gesture, for the Preferences page and Help.
    pub(crate) fn summary(self) -> String {
        use crate::i18n::ctr;
        match self {
            ControlsLayout::Breadslicer => ctr!(
                "Controls",
                "Orbit: middle or right drag · Pan: Shift + middle or right · Zoom: wheel or Ctrl + middle"
            ),
            ControlsLayout::ClassicSlicer => ctr!(
                "Controls",
                "Orbit: right drag (or Shift + middle) · Pan: middle drag · Zoom: wheel or Ctrl + middle"
            ),
            ControlsLayout::Fusion => ctr!(
                "Controls",
                "Orbit: Shift + middle drag · Pan: middle drag · Zoom: wheel"
            ),
            ControlsLayout::SolidWorks => ctr!(
                "Controls",
                "Orbit: middle drag · Pan: Ctrl + middle · Zoom: wheel or Shift + middle"
            ),
            ControlsLayout::Maya => ctr!(
                "Controls",
                "Orbit: Alt + left drag · Pan: Alt + middle (or middle) · Zoom: Alt + right or wheel"
            ),
        }
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_layout_orbits_pans_and_has_its_place_in_the_combo() {
        for layout in ControlsLayout::ALL {
            let m = layout.nav_map();
            let all = [
                m.middle,
                m.middle_shift,
                m.middle_ctrl,
                m.middle_alt,
                m.right,
                m.right_shift,
                m.right_ctrl,
                m.right_alt,
                m.left_alt,
            ];
            assert!(all.contains(&ORBIT), "{layout:?} orbits");
            assert!(all.contains(&PAN), "{layout:?} pans");
            assert_eq!(
                ControlsLayout::from_combo_index(layout.combo_index()),
                layout
            );
        }
        // The tester's ask: the middle button pans in the classic layout.
        assert_eq!(ControlsLayout::ClassicSlicer.nav_map().middle, PAN);
        assert_eq!(ControlsLayout::Breadslicer.nav_map().middle, ORBIT);
    }
}
