/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The generic machine: the printer id whose calibration block on a
//! material applies to any printer without a block of its own.

use std::sync::LazyLock;

use sc_print_primitives::PrinterId;

/// Id of the generic machine profile. A material's calibration block
/// under this id applies to any printer that has no block of its own.
pub fn generic_printer_id() -> &'static PrinterId {
    static ID: LazyLock<PrinterId> =
        LazyLock::new(|| PrinterId::new("generic-machine").expect("valid slug"));
    &ID
}
