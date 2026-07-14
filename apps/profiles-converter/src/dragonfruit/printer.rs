/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mapping of a DragonFruit printer record into a [`PrinterProfile`].
//!
//! The record is deserialised into [`RawPrinter`], a thin mirror of
//! DragonFruit's schema, then [`map_printer`] constructs our domain
//! type. Constructing the domain type is what enforces every invariant
//! (slug grammar, non-empty strings, positive-finite geometry, and the
//! build-volume / pixel-grid envelope agreement), so a value that comes
//! out of here is already valid.

use std::fmt;

use nonempty_collections::NEVec;
use sc_print_primitives::{
    AxisOutOfRange, BuildVolume, FormatKind, InvalidProfileId, Mirror, NetworkProtocol,
    NonEmptyString, OutputFormatBinding, PixelAxisOutOfRange, PixelResolution, PixelSizeMicrons,
    PrinterId, ZeroPixelExtentError,
};
use sc_print_profiles::{
    EnvelopeAxisMismatch, PrinterCapabilities, PrinterConstraints, PrinterHardware,
    PrinterIdentity, PrinterProfile,
};
use semver::Version;
use serde::Deserialize;

/// A DragonFruit printer record, as it appears in the source JSON.
/// Only the fields we map are declared; the rest are ignored.
#[derive(Debug, Deserialize)]
pub(crate) struct RawPrinter {
    #[serde(rename = "presetId")]
    preset_id: String,
    #[serde(rename = "profileVersion")]
    profile_version: u64,
    manufacturer: String,
    #[serde(default)]
    family: Option<String>,
    name: String,
    #[serde(rename = "antiAliasing", default)]
    anti_aliasing: bool,
    #[serde(rename = "hasCamera", default)]
    has_camera: bool,
    #[serde(rename = "networkSupport", default)]
    network_support: Option<String>,
    #[serde(rename = "buildVolumeMm")]
    build_volume_mm: RawBuildVolume,
    #[serde(rename = "pixelSize", default)]
    pixel_size: Option<RawPixelSize>,
    display: RawDisplay,
}

#[derive(Debug, Deserialize)]
struct RawBuildVolume {
    #[serde(default)]
    width: Option<f64>,
    #[serde(default)]
    depth: Option<f64>,
    height: f64,
}

#[derive(Debug, Deserialize)]
struct RawPixelSize {
    x: f64,
    y: f64,
}

#[derive(Debug, Deserialize)]
struct RawDisplay {
    #[serde(rename = "resolutionX")]
    resolution_x: u32,
    #[serde(rename = "resolutionY")]
    resolution_y: u32,
    #[serde(rename = "outputFormat")]
    output_format: String,
    #[serde(rename = "formatVersion", default)]
    format_version: Option<String>,
    #[serde(rename = "mirrorX", default)]
    mirror_x: bool,
    #[serde(rename = "mirrorY", default)]
    mirror_y: bool,
}

/// Everything that can go wrong mapping one DragonFruit printer record.
#[derive(Debug)]
pub enum PrinterMapError {
    InvalidId {
        id: String,
        source: InvalidProfileId,
    },
    EmptyField {
        id: String,
        field: &'static str,
    },
    MissingGeometry {
        id: String,
        axis: &'static str,
    },
    BuildVolume {
        id: String,
        source: AxisOutOfRange,
    },
    PixelResolution {
        id: String,
        source: ZeroPixelExtentError,
    },
    PixelSize {
        id: String,
        source: PixelAxisOutOfRange,
    },
    Envelope {
        id: String,
        source: EnvelopeAxisMismatch,
    },
    EmptyOutputFormat {
        id: String,
    },
}

impl fmt::Display for PrinterMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId { id, source } => {
                write!(f, "printer {id:?}: invalid preset id: {source}")
            }
            Self::EmptyField { id, field } => {
                write!(f, "printer {id:?}: field {field:?} is empty")
            }
            Self::MissingGeometry { id, axis } => write!(
                f,
                "printer {id:?}: cannot determine the {axis} axis; neither build volume nor pixel size is given",
            ),
            Self::BuildVolume { id, source } => write!(f, "printer {id:?}: {source}"),
            Self::PixelResolution { id, source } => write!(f, "printer {id:?}: {source}"),
            Self::PixelSize { id, source } => write!(f, "printer {id:?}: {source}"),
            Self::Envelope { id, source } => write!(f, "printer {id:?}: {source}"),
            Self::EmptyOutputFormat { id } => write!(f, "printer {id:?}: output format is empty"),
        }
    }
}

impl std::error::Error for PrinterMapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidId { source, .. } => Some(source),
            Self::BuildVolume { source, .. } => Some(source),
            Self::PixelResolution { source, .. } => Some(source),
            Self::PixelSize { source, .. } => Some(source),
            Self::Envelope { source, .. } => Some(source),
            Self::EmptyField { .. }
            | Self::MissingGeometry { .. }
            | Self::EmptyOutputFormat { .. } => None,
        }
    }
}

/// Convert one DragonFruit printer record into a [`PrinterProfile`].
pub(crate) fn map_printer(raw: RawPrinter) -> Result<PrinterProfile, PrinterMapError> {
    let id = PrinterId::new(&raw.preset_id).map_err(|source| PrinterMapError::InvalidId {
        id: raw.preset_id.clone(),
        source,
    })?;
    let raw_id = raw.preset_id;

    let manufacturer = non_empty(&raw.manufacturer, &raw_id, "manufacturer")?;
    let name = non_empty(&raw.name, &raw_id, "name")?;
    let family = match raw.family {
        Some(ref value) if !value.is_empty() => non_empty(value, &raw_id, "family")?,
        _ => name.clone(),
    };

    let resolution = PixelResolution::new(raw.display.resolution_x, raw.display.resolution_y)
        .map_err(|source| PrinterMapError::PixelResolution {
            id: raw_id.clone(),
            source,
        })?;

    let pixel = raw.pixel_size;
    let (width_mm, pixel_x_um) = resolve_axis(
        raw.build_volume_mm.width,
        pixel.as_ref().map(|p| p.x),
        raw.display.resolution_x,
    )
    .ok_or_else(|| PrinterMapError::MissingGeometry {
        id: raw_id.clone(),
        axis: "x",
    })?;
    let (depth_mm, pixel_y_um) = resolve_axis(
        raw.build_volume_mm.depth,
        pixel.as_ref().map(|p| p.y),
        raw.display.resolution_y,
    )
    .ok_or_else(|| PrinterMapError::MissingGeometry {
        id: raw_id.clone(),
        axis: "y",
    })?;

    let build_volume =
        BuildVolume::new(width_mm, depth_mm, raw.build_volume_mm.height).map_err(|source| {
            PrinterMapError::BuildVolume {
                id: raw_id.clone(),
                source,
            }
        })?;
    let pixel_size = PixelSizeMicrons::new(pixel_x_um, pixel_y_um).map_err(|source| {
        PrinterMapError::PixelSize {
            id: raw_id.clone(),
            source,
        }
    })?;
    let mirror = Mirror {
        x: raw.display.mirror_x,
        y: raw.display.mirror_y,
    };
    let hardware =
        PrinterHardware::new(build_volume, resolution, pixel_size, mirror).map_err(|source| {
            PrinterMapError::Envelope {
                id: raw_id.clone(),
                source,
            }
        })?;

    let output_binding = map_output_format(&raw.display.output_format, raw.display.format_version)
        .ok_or_else(|| PrinterMapError::EmptyOutputFormat { id: raw_id.clone() })?;
    let capabilities = PrinterCapabilities {
        output_formats: NEVec::new(output_binding),
        supports_grayscale: raw.anti_aliasing,
        two_stage_lift: false,
        transition_layers: None,
        has_camera: raw.has_camera,
        network_protocol: map_network(raw.network_support.as_deref()),
    };

    Ok(PrinterProfile {
        identity: PrinterIdentity {
            id,
            manufacturer,
            family,
            name,
            profile_version: Version::new(raw.profile_version, 0, 0),
            image_asset_path: None,
        },
        hardware,
        capabilities,
        constraints: PrinterConstraints {
            z_step_mm: None,
            layer_height_range_mm: None,
            lift_speed_range_mm_min: None,
            retract_speed_range_mm_min: None,
            lift_distance_max_mm: None,
            light_intensity_mw_per_cm2: None,
        },
    })
}

/// Build a non-empty string for `field`, or report it empty.
fn non_empty(
    value: &str,
    id: &str,
    field: &'static str,
) -> Result<NonEmptyString, PrinterMapError> {
    NonEmptyString::new(value).map_err(|_| PrinterMapError::EmptyField {
        id: id.to_owned(),
        field,
    })
}

/// Resolve one axis to `(build_mm, pixel_um)`. When only one of the two
/// is given, the other is derived from the pixel grid; when neither is
/// given, the axis cannot be determined.
fn resolve_axis(
    build_mm: Option<f64>,
    pixel_um: Option<f64>,
    resolution: u32,
) -> Option<(f64, f64)> {
    match (build_mm, pixel_um) {
        (Some(build), Some(pixel)) => Some((build, pixel)),
        (Some(build), None) => Some((build, build / f64::from(resolution) * 1000.0)),
        (None, Some(pixel)) => Some((f64::from(resolution) * pixel / 1000.0, pixel)),
        (None, None) => None,
    }
}

/// Map a DragonFruit `outputFormat` extension and `formatVersion` into
/// an output-format binding. A known SLA extension maps to its typed
/// [`FormatKind`]; anything else is preserved verbatim as
/// [`FormatKind::Unknown`]. An empty extension yields `None`.
fn map_output_format(output_format: &str, version: Option<String>) -> Option<OutputFormatBinding> {
    let ext = output_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if ext.is_empty() {
        return None;
    }
    let format = match ext.as_str() {
        "sl1" => FormatKind::Sl1,
        "ctb" => FormatKind::Ctb,
        "goo" => FormatKind::Goo,
        "pwmx" => FormatKind::Pwmx,
        other => FormatKind::Unknown(other.to_owned()),
    };
    let version = version.and_then(|value| NonEmptyString::new(&value).ok());
    Some(OutputFormatBinding { format, version })
}

/// Map DragonFruit's `networkSupport` token. `sdcp` is typed; a null or
/// empty value means no network control; anything else is preserved
/// verbatim.
fn map_network(support: Option<&str>) -> Option<NetworkProtocol> {
    match support {
        None | Some("") => None,
        Some("sdcp") => Some(NetworkProtocol::Sdcp),
        Some(other) => Some(NetworkProtocol::Unknown(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(json: &str) -> Result<PrinterProfile, PrinterMapError> {
        map_printer(serde_json::from_str(json).unwrap())
    }

    /// A record with build volume and pixel size both present and
    /// mutually consistent (1000 px x 100 um = 100 mm).
    const FULL: &str = r#"{
        "presetId": "test-printer",
        "profileVersion": 1,
        "manufacturer": "TestCorp",
        "family": "Series",
        "name": "Test Printer",
        "antiAliasing": true,
        "hasCamera": true,
        "networkSupport": "sdcp",
        "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 50.0 },
        "pixelSize": { "x": 100.0, "y": 100.0 },
        "display": {
            "resolutionX": 1000, "resolutionY": 1000,
            "outputFormat": ".ctb", "formatVersion": "v5enc",
            "mirrorX": false, "mirrorY": true
        }
    }"#;

    #[test]
    fn maps_every_field_of_a_full_record() {
        let profile = map(FULL).unwrap();

        assert_eq!(profile.identity.id, PrinterId::new("test-printer").unwrap());
        assert_eq!(profile.identity.manufacturer.as_str(), "TestCorp");
        assert_eq!(profile.identity.family.as_str(), "Series");
        assert_eq!(profile.identity.name.as_str(), "Test Printer");
        assert_eq!(profile.identity.profile_version, Version::new(1, 0, 0));
        assert_eq!(profile.identity.image_asset_path, None);

        assert_eq!(
            profile.hardware.build_volume(),
            BuildVolume::new(100.0, 100.0, 50.0).unwrap(),
        );
        assert_eq!(
            profile.hardware.pixel_resolution(),
            PixelResolution::new(1000, 1000).unwrap(),
        );
        assert_eq!(
            profile.hardware.pixel_size_microns(),
            PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        );
        assert_eq!(profile.hardware.mirror(), Mirror { x: false, y: true });

        assert_eq!(
            *profile.capabilities.output_formats.first(),
            OutputFormatBinding {
                format: FormatKind::Ctb,
                version: Some(NonEmptyString::new("v5enc").unwrap()),
            },
        );
        assert!(profile.capabilities.supports_grayscale);
        assert!(!profile.capabilities.two_stage_lift);
        assert_eq!(profile.capabilities.transition_layers, None);
        assert!(profile.capabilities.has_camera);
        assert_eq!(
            profile.capabilities.network_protocol,
            Some(NetworkProtocol::Sdcp),
        );

        assert_eq!(
            profile.constraints,
            PrinterConstraints {
                z_step_mm: None,
                layer_height_range_mm: None,
                lift_speed_range_mm_min: None,
                retract_speed_range_mm_min: None,
                lift_distance_max_mm: None,
                light_intensity_mw_per_cm2: None,
            }
        );
    }

    #[test]
    fn derives_build_volume_when_it_is_null() {
        let json = r#"{
            "presetId": "null-volume", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Null Volume",
            "buildVolumeMm": { "width": null, "depth": null, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 2000, "outputFormat": ".goo" }
        }"#;
        let profile = map(json).unwrap();
        assert_eq!(
            profile.hardware.build_volume(),
            BuildVolume::new(100.0, 200.0, 40.0).unwrap(),
        );
    }

    #[test]
    fn derives_pixel_size_when_it_is_absent() {
        let json = r#"{
            "presetId": "no-pixel", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "No Pixel",
            "buildVolumeMm": { "width": 100.0, "depth": 200.0, "height": 40.0 },
            "display": { "resolutionX": 1000, "resolutionY": 2000, "outputFormat": ".goo" }
        }"#;
        let profile = map(json).unwrap();
        assert_eq!(
            profile.hardware.pixel_size_microns(),
            PixelSizeMicrons::new(100.0, 100.0).unwrap(),
        );
    }

    #[test]
    fn missing_build_volume_and_pixel_size_is_an_error() {
        let json = r#"{
            "presetId": "no-geometry", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "No Geometry",
            "buildVolumeMm": { "width": null, "depth": null, "height": 40.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            PrinterMapError::MissingGeometry { axis: "x", .. },
        ));
    }

    #[test]
    fn family_falls_back_to_name_when_absent() {
        let json = r#"{
            "presetId": "no-family", "profileVersion": 1,
            "manufacturer": "TestCorp", "name": "Lonely Printer",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let profile = map(json).unwrap();
        assert_eq!(profile.identity.family.as_str(), "Lonely Printer");
    }

    #[test]
    fn unknown_output_extension_is_preserved_verbatim() {
        let json = r#"{
            "presetId": "azf-printer", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "AZF Printer",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".azf" }
        }"#;
        let profile = map(json).unwrap();
        assert_eq!(
            *profile.capabilities.output_formats.first(),
            OutputFormatBinding {
                format: FormatKind::Unknown("azf".to_owned()),
                version: None,
            },
        );
    }

    #[test]
    fn network_support_maps_null_and_unknown() {
        assert_eq!(map_network(None), None);
        assert_eq!(map_network(Some("")), None);
        assert_eq!(map_network(Some("sdcp")), Some(NetworkProtocol::Sdcp));
        assert_eq!(
            map_network(Some("nanodlp")),
            Some(NetworkProtocol::Unknown("nanodlp".to_owned())),
        );
    }

    #[test]
    fn anti_aliasing_and_camera_flags_carry_over() {
        let json = r#"{
            "presetId": "flags", "profileVersion": 2,
            "manufacturer": "TestCorp", "family": "Series", "name": "Flags",
            "antiAliasing": false, "hasCamera": false,
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let profile = map(json).unwrap();
        assert!(!profile.capabilities.supports_grayscale);
        assert!(!profile.capabilities.has_camera);
        assert_eq!(profile.capabilities.network_protocol, None);
        assert_eq!(profile.identity.profile_version, Version::new(2, 0, 0));
    }

    #[test]
    fn an_invalid_preset_id_is_rejected() {
        let json = r#"{
            "presetId": "Bad_Id", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Bad",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            PrinterMapError::InvalidId { .. },
        ));
    }

    #[test]
    fn build_volume_disagreeing_with_the_pixel_grid_is_rejected() {
        // 500 px x 100 um = 50 mm, but the record claims 100 mm.
        let json = r#"{
            "presetId": "mismatch", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Mismatch",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 500, "resolutionY": 500, "outputFormat": ".goo" }
        }"#;
        assert!(matches!(
            map(json).unwrap_err(),
            PrinterMapError::Envelope { .. },
        ));
    }
}
