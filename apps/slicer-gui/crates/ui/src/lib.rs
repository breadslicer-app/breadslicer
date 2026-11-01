/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Rust code that Slint generates from `apps/slicer-gui/ui/main-window.slint`:
//! the windows, dialogs and data types of the interface. It depends on the
//! `.slint` files and the translations alone, so it compiles again only when
//! one of them changes.

// Slint's generated code allows the clippy groups by itself, but not the rustc
// lints the workspace denies. Lint attributes are ignored on macro invocations,
// hence the wrapper module.
#[allow(missing_debug_implementations, unreachable_pub, unused_qualifications)]
mod generated {
    slint::include_modules!();
}

pub use generated::*;
