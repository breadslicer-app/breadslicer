/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The startup-notification token a desktop launcher hands to the
//! process it starts. Spending it on the first window is what tells
//! the shell the launch is over; a token left unused leaves the launch
//! pending, and the shell shows a busy pointer until it times out.

// `StartupToken` itself stays unexported: the value only travels from
// `take_from_env` to `attach`, and neither platform's type carries
// anything a caller could ask it.
#[cfg(all(unix, not(target_vendor = "apple")))]
pub(crate) use unix::{attach, take_from_env};

#[cfg(not(all(unix, not(target_vendor = "apple"))))]
pub(crate) use other::{attach, take_from_env};

#[cfg(all(unix, not(target_vendor = "apple")))]
mod unix {
    use std::cell::Cell;
    use std::env;

    use slint::BackendSelector;
    use slint::winit_030::winit::platform::startup_notify::WindowAttributesExtStartupNotify;
    use slint::winit_030::winit::platform::startup_notify::reset_activation_token_env;
    use slint::winit_030::winit::window::ActivationToken;

    /// The launch a desktop shell is waiting on, when one started this
    /// process.
    #[derive(Debug, Default)]
    pub(crate) struct StartupToken(Option<ActivationToken>);

    /// Takes the launcher's token out of the environment.
    ///
    /// Call while the process is still single-threaded: this clears
    /// the variables, and removing an environment variable races every
    /// other thread reading the environment.
    pub(crate) fn take_from_env() -> StartupToken {
        // Wayland carries the token in XDG_ACTIVATION_TOKEN and X11 in
        // DESKTOP_STARTUP_ID; a session sets only its own.
        let token = env::var("XDG_ACTIVATION_TOKEN")
            .or_else(|_| env::var("DESKTOP_STARTUP_ID"))
            .ok()
            .map(ActivationToken::from_raw);
        // Clearing them stops a child process from claiming a launch
        // that was never about it.
        reset_activation_token_env();
        StartupToken(token)
    }

    /// Spends the token on the first window the backend creates,
    /// retiring the launch that the shell opened.
    pub(crate) fn attach(selector: BackendSelector, token: StartupToken) -> BackendSelector {
        let StartupToken(Some(token)) = token else {
            return selector;
        };
        // The hook runs per window, while a token names one launch.
        let token = Cell::new(Some(token));
        selector.with_winit_window_attributes_hook(move |attributes| match token.take() {
            Some(token) => attributes.with_activation_token(token),
            None => attributes,
        })
    }
}

#[cfg(not(all(unix, not(target_vendor = "apple"))))]
mod other {
    use slint::BackendSelector;

    /// Nothing to spend: startup tokens are an xdg desktop mechanism.
    #[derive(Debug, Default)]
    pub(crate) struct StartupToken;

    /// Reports that no launch is pending.
    pub(crate) fn take_from_env() -> StartupToken {
        StartupToken
    }

    /// Leaves the backend selection untouched.
    pub(crate) fn attach(selector: BackendSelector, _token: StartupToken) -> BackendSelector {
        selector
    }
}
