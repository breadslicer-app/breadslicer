/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Drags of the Supports tab's tools (#3): a tip moved over the model, a
//! box or a brush over the supports to remove. The viewport tells the support
//! driver when one starts, moves and ends, with what lies under the cursor;
//! the driver previews and makes the edit.
//!
//! A press with one of these tools armed is a click until the cursor moves
//! past the click threshold; only then does the drag start. Esc or a right
//! press drops a drag in flight; the left release that ends it then does
//! nothing.

/// What a drag of a Supports tab tool does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupportDragKind {
    /// Move tip: the tip grabbed follows the cursor over the model.
    MoveTip,
    /// Remove, box: every tip inside the box from the press to the cursor.
    Box,
    /// Remove, brush: every tip the brush passes over.
    Brush,
}

/// Where the cursor is during a drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DragPoint {
    /// The cursor, in logical pixels of the viewport.
    pub(crate) cursor: [f32; 2],
    /// The model surface under it (world mm), looking through the supports.
    pub(crate) model: Option<[f32; 3]>,
    /// The supports' surface under it (world mm), even behind the model.
    pub(crate) support: Option<[f32; 3]>,
}

/// What the camera sees first through a pixel: the leaf (a model or a set
/// of supports) and the point (world mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScreenHit {
    pub(crate) leaf: sc_scene::NodeId,
    pub(crate) point: [f32; 3],
}

/// Per pixel, what a ray from the camera through it meets first.
pub(crate) type Hits<'a> = dyn Fn(&[[f32; 2]]) -> Vec<Option<ScreenHit>> + 'a;

/// The view a drag sees the scene through.
pub(crate) struct DragView<'a> {
    /// Where a world point lands on the viewport, in logical pixels; `None`
    /// behind the eye.
    pub(crate) project: &'a dyn Fn([f32; 3]) -> Option<[f32; 2]>,
    /// Per pixel, what a ray from the camera through it meets first among
    /// every leaf of the scene; asked many at once, so the rays share the
    /// cores.
    pub(crate) hits: &'a Hits<'a>,
}

impl DragView<'_> {
    pub(crate) fn project(&self, p: [f32; 3]) -> Option<[f32; 2]> {
        (self.project)(p)
    }

    pub(crate) fn hits(&self, pixels: &[[f32; 2]]) -> Vec<Option<ScreenHit>> {
        (self.hits)(pixels)
    }
}

/// The view a drag's step sees.
pub(crate) type Project<'a> = &'a DragView<'a>;

/// The support driver's side of a drag.
pub(crate) trait SupportDragHandler {
    /// A drag of `kind` starts at `press`: whether the driver takes it. A
    /// drag it does not take does nothing until the release.
    fn drag_start(&self, kind: SupportDragKind, press: &DragPoint, project: Project<'_>) -> bool;
    /// The drag it took moved to `at`.
    fn drag_move(&self, at: &DragPoint, project: Project<'_>);
    /// The drag ended: released (`commit`), or dropped by Esc or a right
    /// press.
    fn drag_end(&self, commit: bool);
}

/// Where a press of a Supports tab tool stands.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) enum DragPhase {
    /// No press of a drag tool.
    #[default]
    Idle,
    /// Pressed at this pixel; still a click.
    Pressed([f32; 2], SupportDragKind),
    /// A drag the driver took.
    Dragging(SupportDragKind),
    /// A drag the driver did not take, or one dropped: nothing until the
    /// release.
    Ignored,
}
