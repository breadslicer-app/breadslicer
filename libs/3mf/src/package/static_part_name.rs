/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::error::MalformedPartNameError;

/// A part name that is known when the code compiles. Declare it as a `const`
/// item that matches on [`StaticPartName::new`] and panics on `Err`. The
/// compiler then checks the name: a malformed one stops the build, and no check
/// or panic happens at run time. `PartName::from` converts it. The
/// [`package`](super) module documentation shows an example.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticPartName(&'static str);

impl StaticPartName {
    /// Returns `name` as a part name, or the error of the first part-name rule
    /// that it breaks.
    pub const fn new(name: &'static str) -> Result<Self, MalformedPartNameError> {
        match check_part_name(name) {
            Ok(()) => Ok(Self(name)),
            Err(error) => Err(error),
        }
    }

    #[inline]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Checks `name` against the part-name rules: not empty, starts with `/`, no
/// empty segment, and no `.` at the end. A `const fn`, so that
/// [`StaticPartName::new`] can run it when the code compiles.
pub(super) const fn check_part_name(name: &str) -> Result<(), MalformedPartNameError> {
    // Bytes suffice: `/` and `.` are ASCII, and no byte of a multi-byte UTF-8
    // character is ASCII.
    let bytes = name.as_bytes();
    match bytes {
        [] => return Err(MalformedPartNameError::Empty),
        [b'/', ..] => {}
        _ => return Err(MalformedPartNameError::NotAbsolute),
    }
    let mut rest = bytes;
    while let [byte, tail @ ..] = rest {
        if *byte == b'/' && matches!(tail, [] | [b'/', ..]) {
            return Err(MalformedPartNameError::EmptySegment);
        }
        rest = tail;
    }
    if let [.., b'.'] = bytes {
        return Err(MalformedPartNameError::TrailingDot);
    }
    Ok(())
}
