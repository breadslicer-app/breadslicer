/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Why a window's viewport has no graphics device.
//!
//! The device itself lives in [`crate::scene_loader::SceneLoader`], published
//! from `RenderingSetup` and cleared on `RenderingTeardown`. An `Option` over
//! that slot answers "is there a device", but not "will there ever be one", and
//! those 2 cases call for opposite handling: a setup that has not happened yet
//! is worth waiting for, while a setup that arrived carrying a graphics API
//! this build cannot use, or whose device refused the viewport renderer, will
//! not improve on its own.
//!
//! This type carries that distinction, along with the wording the user sees.
//! Nothing here decides whether a device exists; the loader derives that from
//! the device slot and reaches for this only when the slot is empty.

use std::cell::RefCell;
use std::fmt;

use crate::i18n::ctr;
use crate::mesh_render::StageRefusal;

/// The reason the viewport has no graphics device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum GraphicsUnavailable {
    /// No `RenderingSetup` has published a device yet, or a teardown took the
    /// last one away. Slint delivers `RenderingSetup` on the window's first
    /// paint rather than at startup, so this is the ordinary state between
    /// opening a window and painting it, and a device may still arrive.
    #[default]
    AwaitingSetup,
    /// A `RenderingSetup` arrived carrying a graphics API this build cannot
    /// render through, named as Slint reported it. The viewport stays empty for
    /// as long as that renderer is in use.
    UnsupportedApi(String),
    /// A `RenderingSetup` published a device, but the device refused one of the
    /// viewport renderer's resources; the refusal names which one and why. A
    /// deterministic refusal repeats on the same device, so the window is left
    /// without a viewport.
    RendererRefused(StageRefusal),
    /// The shared device was lost (driver reset, device removed, destroyed).
    /// Nothing that needs a device works again in this process.
    DeviceLost,
}

impl GraphicsUnavailable {
    /// Whether a device can still turn up, which decides between parking a
    /// request until it does and telling the user it never will.
    pub(crate) fn may_still_arrive(&self) -> bool {
        match self {
            Self::AwaitingSetup => true,
            Self::UnsupportedApi(_) | Self::RendererRefused(_) | Self::DeviceLost => false,
        }
    }

    /// What to tell the user when this blocks something they asked for. Split
    /// by variant because the 2 cases warrant opposite advice: one is worth
    /// retrying, the other is not.
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
            Self::RendererRefused(reason) => ctr!(
                "ErrorDialog",
                "{} Files cannot be opened in this window.",
                reason
            )
            .to_string(),
            Self::DeviceLost => ctr!(
                "ErrorDialog",
                "The graphics device was lost. Save your work and restart the application."
            )
            .to_string(),
        }
    }
}

/// The text is [`GraphicsUnavailable::message`], in the language in force each
/// time it is formatted.
impl fmt::Display for GraphicsUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for GraphicsUnavailable {}

/// Tracks the reason across the rendering lifecycle. Held by the loader
/// alongside the device slot; the loader keeps the 2 in step by driving this
/// from the same calls that fill and empty the slot.
///
/// Losing a device is deliberately not a transition here: whether a device
/// exists is the slot's business, and this only speaks when the slot is empty.
/// Publishing one is a transition, because it retires whatever the previous
/// setup was blamed for.
#[derive(Debug, Default)]
pub(crate) struct GraphicsGap {
    reason: RefCell<GraphicsUnavailable>,
}

impl GraphicsGap {
    /// The current reason, valid only while no device is published.
    pub(crate) fn reason(&self) -> GraphicsUnavailable {
        self.reason.borrow().clone()
    }

    /// A setup published a usable device. Clears any earlier complaint so a
    /// later teardown does not report a renderer that is no longer in use.
    pub(crate) fn note_device_published(&self) {
        *self.reason.borrow_mut() = GraphicsUnavailable::AwaitingSetup;
    }

    /// A setup arrived carrying a graphics API this build cannot render
    /// through.
    pub(crate) fn note_unsupported_api(&self, api: String) {
        *self.reason.borrow_mut() = GraphicsUnavailable::UnsupportedApi(api);
    }

    /// A setup's device refused the viewport renderer.
    pub(crate) fn note_renderer_refused(&self, refusal: StageRefusal) {
        *self.reason.borrow_mut() = GraphicsUnavailable::RendererRefused(refusal);
    }

    pub(crate) fn note_device_lost(&self) {
        *self.reason.borrow_mut() = GraphicsUnavailable::DeviceLost;
    }
}

#[cfg(test)]
mod graphics_gap_tests {
    use super::*;
    use crate::mesh_render::RendererStage;

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
        // A window renders through 1 API at a time, so the newest setup is the
        // one describing the current state.
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
        // A teardown after a working setup has to read as "waiting", not as the
        // unusable renderer from 2 setups ago, otherwise a recoverable state
        // would be reported as a dead end.
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

    #[test]
    fn a_lost_device_is_recorded() {
        let gap = GraphicsGap::default();
        gap.note_device_published();
        gap.note_device_lost();
        assert_eq!(gap.reason(), GraphicsUnavailable::DeviceLost);
    }

    #[test]
    fn a_refused_renderer_is_recorded_and_retired_by_a_later_device() {
        let gap = GraphicsGap::default();
        let refusal = StageRefusal {
            stage: RendererStage::Mesh,
            cause: "Validation Error: x".to_string(),
        };
        gap.note_renderer_refused(refusal.clone());
        assert_eq!(gap.reason(), GraphicsUnavailable::RendererRefused(refusal));
        gap.note_device_published();
        assert_eq!(gap.reason(), GraphicsUnavailable::AwaitingSetup);
    }
}

#[cfg(test)]
mod graphics_unavailable_tests {
    use super::*;
    use crate::mesh_render::RendererStage;

    /// The reason of a window whose device refused the mesh pipeline.
    fn refused_renderer() -> GraphicsUnavailable {
        GraphicsUnavailable::RendererRefused(StageRefusal {
            stage: RendererStage::Mesh,
            cause: "Validation Error: x".to_string(),
        })
    }

    #[test]
    fn the_default_is_awaiting_setup() {
        // A freshly built loader has seen no setup at all, which is the state a
        // request should wait through rather than report as a failure.
        assert_eq!(
            GraphicsUnavailable::default(),
            GraphicsUnavailable::AwaitingSetup
        );
    }

    #[test]
    fn a_device_may_still_arrive_while_awaiting_setup() {
        assert!(GraphicsUnavailable::AwaitingSetup.may_still_arrive());
    }

    #[test]
    fn no_device_is_coming_for_an_unsupported_api() {
        // Waiting through this one would park a request forever, since the
        // renderer in use does not change on its own.
        assert!(
            !GraphicsUnavailable::UnsupportedApi("GraphicsAPI::NativeOpenGL".to_string())
                .may_still_arrive()
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
        // unreproducible machine actionable, so it has to reach the dialog.
        let message =
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::NativeOpenGL".to_string()).message();
        assert!(
            message.contains("GraphicsAPI::NativeOpenGL"),
            "the renderer must be named: {message}"
        );
    }

    #[test]
    fn the_two_reasons_do_not_share_wording() {
        // Both end up in the same dialog slot, so identical wording would erase
        // the distinction this type exists to carry.
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

    #[test]
    fn no_device_is_coming_for_a_refused_renderer() {
        assert!(!refused_renderer().may_still_arrive());
    }

    #[test]
    fn no_device_is_coming_after_a_loss_and_the_message_says_to_restart() {
        assert!(!GraphicsUnavailable::DeviceLost.may_still_arrive());
        let message = GraphicsUnavailable::DeviceLost.message();
        assert!(message.contains("lost"), "{message}");
        assert!(message.contains("restart"), "{message}");
        assert_ne!(message, GraphicsUnavailable::AwaitingSetup.message());
    }

    #[test]
    fn a_refused_renderer_carries_the_reason_and_says_files_cannot_open() {
        let message = refused_renderer().message();
        assert!(message.contains("mesh pipeline"), "{message}");
        assert!(message.contains("cannot be opened"), "{message}");
        assert!(!message.contains("again"), "{message}");
        assert_ne!(message, GraphicsUnavailable::AwaitingSetup.message());
        assert_ne!(
            message,
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::WebGL".to_string()).message()
        );
    }

    #[test]
    fn the_text_of_each_reason_is_its_message() {
        for reason in [
            GraphicsUnavailable::AwaitingSetup,
            GraphicsUnavailable::UnsupportedApi("GraphicsAPI::WebGL".to_string()),
            refused_renderer(),
            GraphicsUnavailable::DeviceLost,
        ] {
            assert_eq!(reason.to_string(), reason.message());
        }
    }
}
