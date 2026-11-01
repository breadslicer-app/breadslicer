/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The translation macro of the GUI crates.

/// Translates a message with a disambiguation context (the `msgctxt` of a `.po`
/// file) into the language that Slint has selected, and puts the arguments into
/// it.
///
/// Each crate that calls it has its own catalog, which its build script
/// writes into its `crate::i18n` module from its own calls (see
/// `sc_message_catalog::generate`), so a call with no entry does not compile. On a thread with no Slint context, such as a worker
/// thread, the result is the source text.
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
/// xgettext -L Rust --keyword='ctr!:1c,2' \
///     --keyword='ctr!:1c,2,3,4t' --keyword='ctr!:1c,2,3,5t' ... \
///     --keyword='ctr!:1c,2,3,10t' ...
/// ```
// `crate` and not `$crate`: the catalog is the calling crate's.
#[allow(clippy::crate_in_macro_def)]
#[macro_export]
macro_rules! ctr {
    // Plural, with or without format arguments.
    ($ctx:literal, $singular:literal, $plural:literal, $n:expr $(, $arg:expr)* $(,)?) => {{
        const MESSAGE: &crate::i18n::PluralMessage =
            match crate::i18n::plural_message($ctx, $singular, $plural) {
                Some(message) => message,
                None => panic!(
                    "{}",
                    concat!(
                        "no catalog entry for ctr!(",
                        stringify!($ctx),
                        ", ",
                        stringify!($singular),
                        ", ",
                        stringify!($plural),
                        ", ..)"
                    )
                ),
            };
        let args: &[slint::SharedString] = &[
            $( slint::SharedString::from(std::format!("{}", $arg)) ),*
        ];
        slint::private_unstable_api::re_exports::translate_from_bundle_with_plural(
            MESSAGE.translations,
            crate::i18n::PLURAL_RULES,
            args,
            $n as i32,
        )
    }};
    // Singular, with or without format arguments.
    ($ctx:literal, $msgid:literal $(, $arg:expr)* $(,)?) => {{
        const MESSAGE: &crate::i18n::Message = match crate::i18n::message($ctx, $msgid) {
            Some(message) => message,
            None => panic!(
                "{}",
                concat!(
                    "no catalog entry for ctr!(",
                    stringify!($ctx),
                    ", ",
                    stringify!($msgid),
                    ", ..)"
                )
            ),
        };
        let args: &[slint::SharedString] = &[
            $( slint::SharedString::from(std::format!("{}", $arg)) ),*
        ];
        slint::private_unstable_api::re_exports::translate_from_bundle(MESSAGE.translations, args)
    }};
}
