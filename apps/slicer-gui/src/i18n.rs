/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

/// Translate a message string.
///
/// Calls `slint::private_unstable_api::translate` under the hood so that
/// Rust-side strings go through the same gettext / bundled-translation
/// pipeline as `.slint` files.
///
/// # Forms
///
/// ```ignore
/// tr!("Hello world")
/// tr!("Hello {}", name)
/// tr!("One item", "{n} items", count)
/// tr!("One {} item", "{n} {} items", count, kind)
/// ```
///
/// # xgettext
///
/// ```sh
/// xgettext -L Rust --keyword='tr!:1' --keyword='tr!:1,2' ...
/// ```
#[allow(unused_macros)]
macro_rules! tr {
    // Plural with format args
    ($singular:literal, $plural:literal, $n:expr, $($arg:expr),+ $(,)?) => {{
        let args: &[slint::SharedString] = &[
            $( slint::SharedString::from(std::format!("{}", $arg)) ),+
        ];
        slint::private_unstable_api::translate(
            $singular.into(),
            slint::SharedString::default(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(args),
            $n as i32,
            $plural.into(),
        )
    }};
    // Plural without format args
    ($singular:literal, $plural:literal, $n:expr $(,)?) => {
        slint::private_unstable_api::translate(
            $singular.into(),
            slint::SharedString::default(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(&[]),
            $n as i32,
            $plural.into(),
        )
    };
    // Simple with format args
    ($msgid:literal, $($arg:expr),+ $(,)?) => {{
        let args: &[slint::SharedString] = &[
            $( slint::SharedString::from(std::format!("{}", $arg)) ),+
        ];
        slint::private_unstable_api::translate(
            $msgid.into(),
            slint::SharedString::default(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(args),
            0_i32,
            slint::SharedString::default(),
        )
    }};
    // Simple without format args
    ($msgid:literal $(,)?) => {
        slint::private_unstable_api::translate(
            $msgid.into(),
            slint::SharedString::default(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(&[]),
            0_i32,
            slint::SharedString::default(),
        )
    };
}

/// Translate a message string with a disambiguation context.
///
/// Works like [`tr!`] but the first argument is a context string (the
/// `msgctxt` in PO files), used to disambiguate identical source strings
/// that need different translations.
///
/// # Forms
///
/// ```ignore
/// ctr!("MenuBar", "File")
/// ctr!("MenuBar", "Open {}", name)
/// ctr!("MenuBar", "One file", "{n} files", count)
/// ctr!("MenuBar", "One {} file", "{n} {} files", count, kind)
/// ```
///
/// # xgettext
///
/// ```sh
/// xgettext -L Rust --keyword='ctr!:1c,2' --keyword='ctr!:1c,2,3' ...
/// ```
macro_rules! ctr {
    // Plural with format args
    ($ctx:literal, $singular:literal, $plural:literal, $n:expr, $($arg:expr),+ $(,)?) => {{
        let args: &[slint::SharedString] = &[
            $( slint::SharedString::from(std::format!("{}", $arg)) ),+
        ];
        slint::private_unstable_api::translate(
            $singular.into(),
            $ctx.into(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(args),
            $n as i32,
            $plural.into(),
        )
    }};
    // Plural without format args
    ($ctx:literal, $singular:literal, $plural:literal, $n:expr $(,)?) => {
        slint::private_unstable_api::translate(
            $singular.into(),
            $ctx.into(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(&[]),
            $n as i32,
            $plural.into(),
        )
    };
    // Simple with format args
    ($ctx:literal, $msgid:literal, $($arg:expr),+ $(,)?) => {{
        let args: &[slint::SharedString] = &[
            $( slint::SharedString::from(std::format!("{}", $arg)) ),+
        ];
        slint::private_unstable_api::translate(
            $msgid.into(),
            $ctx.into(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(args),
            0_i32,
            slint::SharedString::default(),
        )
    }};
    // Simple without format args
    ($ctx:literal, $msgid:literal $(,)?) => {
        slint::private_unstable_api::translate(
            $msgid.into(),
            $ctx.into(),
            slint::SharedString::default(),
            slint::private_unstable_api::re_exports::Slice::from_slice(&[]),
            0_i32,
            slint::SharedString::default(),
        )
    };
}

#[allow(unused_imports)]
pub(crate) use ctr;
#[allow(unused_imports)]
pub(crate) use tr;
