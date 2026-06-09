/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Provenance tag for each resolved field.
//!
//! Names the layer of the resolver pipeline that supplied a final
//! value: a per-printer calibrated entry, a material-level default,
//! a derivation from the material's intent block, or a scene/CLI
//! override.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueSource {
    Calibrated,
    Default,
    Derived,
    Override,
}

impl ValueSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Calibrated => "calibrated",
            Self::Default => "default",
            Self::Derived => "derived",
            Self::Override => "override",
        }
    }
}

impl std::fmt::Display for ValueSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
