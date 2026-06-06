/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Network protocol identifiers for remote-controlled printers.
//!
//! [`NetworkProtocol`] enumerates the known remote-control protocols
//! exposed by SLA printers (Elegoo Centauri, Anycubic Homie, Klipper's
//! Moonraker) plus an `Unknown(String)` escape hatch so profiles that
//! name an unrecognised protocol still parse. Profiles wrap the type
//! in `Option<NetworkProtocol>`; `None` means the printer offers no
//! network control.

/// Network protocol an SLA printer exposes for remote control.
/// `Unknown(String)` is the escape hatch for forward compatibility.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NetworkProtocol {
    ElegooCentauri,
    AnycubicHomie,
    Moonraker,
    Unknown(String),
}

impl NetworkProtocol {
    /// Canonical lowercase identifier used in profile TOML.
    pub fn as_canonical(&self) -> &str {
        match self {
            Self::ElegooCentauri => "elegoo_centauri",
            Self::AnycubicHomie => "anycubic_homie",
            Self::Moonraker => "moonraker",
            Self::Unknown(s) => s,
        }
    }

    /// Parse from the canonical lowercase identifier. Unrecognised
    /// identifiers fall through to [`NetworkProtocol::Unknown`] rather
    /// than failing.
    pub fn from_canonical(s: &str) -> Self {
        match s {
            "elegoo_centauri" => Self::ElegooCentauri,
            "anycubic_homie" => Self::AnycubicHomie,
            "moonraker" => Self::Moonraker,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for NetworkProtocol {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_canonical())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for NetworkProtocol {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_canonical(&s))
    }
}
