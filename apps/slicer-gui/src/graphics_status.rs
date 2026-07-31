/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Why a window's viewport has no graphics device.
//!
//! The device itself lives in [`crate::scene_loader::SceneLoader`],
//! published from `RenderingSetup` and cleared on `RenderingTeardown`.
//! An `Option` over that slot answers "is there a device", but not "will
//! there ever be one", and those two cases call for opposite handling: a
//! setup that has not happened yet is worth waiting for, while a setup
//! that arrived carrying a graphics API this build cannot use will not
//! improve on its own.
//!
//! This type carries that distinction, along with the wording the user
//! sees. Nothing here decides whether a device exists; the loader
//! derives that from the device slot and reaches for this only when the
//! slot is empty.

use std::cell::RefCell;

use crate::i18n::ctr;

/// The reason the viewport has no graphics device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum GraphicsUnavailable {
    /// No `RenderingSetup` has published a device yet, or a teardown
    /// took the last one away. Slint delivers `RenderingSetup` on the
    /// window's first paint rather than at startup, so this is the
    /// ordinary state between opening a window and painting it, and a
    /// device may still arrive.
    #[default]
    AwaitingSetup,
    /// A `RenderingSetup` arrived carrying a graphics API this build
    /// cannot render through, named as Slint reported it. The viewport
    /// stays empty for as long as that renderer is in use.
    UnsupportedApi(String),
}

impl GraphicsUnavailable {
    /// What to tell the user when this blocks something they asked for.
    /// Split by variant because the two cases warrant opposite advice:
    /// one is worth retrying, the other is not.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::AwaitingSetup => ctr!(
                "ErrorDialog",
                "The 3D view is still starting up. Please try again in a moment."
            )
            .to_string(),
            Self::UnsupportedApi(api) => ctr!(
                "ErrorDialog",
                "The 3D view could not start: this window renders through {}, which this build cannot use for 3D.",
                api
            )
            .to_string(),
        }
    }
}

/// Tracks the reason across the rendering lifecycle. Held by the
/// loader alongside the device slot; the loader keeps the two in step
/// by driving this from the same calls that fill and empty the slot.
///
/// Losing a device is deliberately not a transition here: whether a
/// device exists is the slot's business, and this only speaks when the
/// slot is empty. Publishing one is a transition, because it retires
/// whatever the previous setup was blamed for.
#[derive(Debug, Default)]
pub(crate) struct GraphicsGap {
    reason: RefCell<GraphicsUnavailable>,
}

impl GraphicsGap {
    /// The current reason, valid only while no device is published.
    pub(crate) fn reason(&self) -> GraphicsUnavailable {
        self.reason.borrow().clone()
    }

    /// A setup published a usable device. Clears any earlier complaint
    /// so a later teardown does not report a renderer that is no longer
    /// in use.
    pub(crate) fn note_device_published(&self) {
        *self.reason.borrow_mut() = GraphicsUnavailable::AwaitingSetup;
    }

    /// A setup arrived carrying a graphics API this build cannot render
    /// through.
    pub(crate) fn note_unsupported_api(&self, api: String) {
        *self.reason.borrow_mut() = GraphicsUnavailable::UnsupportedApi(api);
    }
}

#[cfg(test)]
mod graphics_gap_tests {
    use super::*;

    #[test]
    fn a_fresh_gap_is_awaiting_setup() {
        assert_eq!(
            GraphicsGap::default().reason(),
            GraphicsUnavailable::AwaitingSetup
        );
    }

    #[test]
    fn an_unsupported_api_is_recorded() {
        let gap = GraphicsGap::default();
        gap.note_unsupported_api("GraphicsAPI::NativeOpenGL".to_string());
        assert_eq!(
            gap.reason(),
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::NativeOpenGL".to_string())
        );
    }

    #[test]
    fn the_latest_unsupported_api_wins() {
        // A window renders through one API at a time, so the newest
        // setup is the one describing the current state.
        let gap = GraphicsGap::default();
        gap.note_unsupported_api("GraphicsAPI::NativeOpenGL".to_string());
        gap.note_unsupported_api("GraphicsAPI::WebGL".to_string());
        assert_eq!(
            gap.reason(),
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::WebGL".to_string())
        );
    }

    #[test]
    fn publishing_a_device_retires_an_earlier_complaint() {
        // A teardown after a working setup has to read as "waiting",
        // not as the unusable renderer from two setups ago, otherwise
        // a recoverable state would be reported as a dead end.
        let gap = GraphicsGap::default();
        gap.note_unsupported_api("GraphicsAPI::NativeOpenGL".to_string());
        gap.note_device_published();
        assert_eq!(gap.reason(), GraphicsUnavailable::AwaitingSetup);
    }

    #[test]
    fn publishing_a_device_twice_stays_awaiting_setup() {
        let gap = GraphicsGap::default();
        gap.note_device_published();
        gap.note_device_published();
        assert_eq!(gap.reason(), GraphicsUnavailable::AwaitingSetup);
    }

    #[test]
    fn an_unsupported_api_after_a_working_setup_is_recorded_again() {
        // The teardown-then-bad-setup order, which is how a window that
        // rendered once can end up without a device for good.
        let gap = GraphicsGap::default();
        gap.note_device_published();
        gap.note_unsupported_api("GraphicsAPI::WebGL".to_string());
        assert_eq!(
            gap.reason(),
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::WebGL".to_string())
        );
    }
}

#[cfg(test)]
mod graphics_unavailable_tests {
    use super::*;

    #[test]
    fn the_default_is_awaiting_setup() {
        // A freshly built loader has seen no setup at all, which is the
        // state a request should wait through rather than report as a
        // failure.
        assert_eq!(
            GraphicsUnavailable::default(),
            GraphicsUnavailable::AwaitingSetup
        );
    }

    #[test]
    fn awaiting_setup_suggests_retrying() {
        let message = GraphicsUnavailable::AwaitingSetup.message();
        assert!(
            message.contains("again"),
            "a wait-and-retry case must say so: {message}"
        );
    }

    #[test]
    fn an_unsupported_api_names_the_renderer() {
        // The API name is the one detail that makes a report from an
        // unreproducible machine actionable, so it has to reach the
        // dialog.
        let message =
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::NativeOpenGL".to_string()).message();
        assert!(
            message.contains("GraphicsAPI::NativeOpenGL"),
            "the renderer must be named: {message}"
        );
    }

    #[test]
    fn the_two_reasons_do_not_share_wording() {
        // Both end up in the same dialog slot, so identical wording
        // would erase the distinction this type exists to carry.
        assert_ne!(
            GraphicsUnavailable::AwaitingSetup.message(),
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::WebGL".to_string()).message()
        );
    }

    #[test]
    fn an_unsupported_api_does_not_suggest_retrying() {
        let message =
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::NativeOpenGL".to_string()).message();
        assert!(
            !message.contains("again"),
            "waiting will not help here, so the wording must not invite it: {message}"
        );
    }
}
