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
    EnvelopeAxisMismatch, PrinterCapabilities, PrinterConstraints, PrinterDefaults,
    PrinterHardware, PrinterIdentity, PrinterProfile, PrinterTimeCalibration,
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
    #[serde(rename = "imageAssetPath", default)]
    image_asset_path: Option<String>,
    #[serde(rename = "antiAliasing", default)]
    anti_aliasing: bool,
    #[serde(rename = "hasCamera", default)]
    has_camera: bool,
    #[serde(rename = "networkSupport", default)]
    network_support: Option<String>,
    #[serde(rename = "bitDepth", default)]
    bit_depth: Option<RawBitDepth>,
    #[serde(rename = "buildVolumeMm")]
    build_volume_mm: RawBuildVolume,
    #[serde(rename = "pixelSize", default)]
    pixel_size: Option<RawPixelSize>,
    display: RawDisplay,
}

/// How many bits a machine's panel drives. The source states it for a
/// few machines only, alongside prose we do not read.
#[derive(Debug, Deserialize)]
struct RawBitDepth {
    bits: u32,
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
    #[serde(rename = "settingsMode", default)]
    settings_mode: Option<String>,
    #[serde(rename = "mirrorX", default)]
    mirror_x: bool,
    #[serde(rename = "mirrorY", default)]
    mirror_y: bool,
}

/// The source's word on how a machine separates a layer.
///
/// It is a statement about the settings the source shows for the
/// machine, not a measurement, so it is reported and never written into
/// a capability: what a machine can do is ours to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Classification {
    /// The platform moves in one go.
    Single,
    /// The platform's travel is split into 2 stages.
    Split,
    /// The vat tilts instead of the platform peeling off it.
    TiltingVat,
    /// A word this converter does not know, kept as the source wrote it
    /// so a run can report it rather than fail.
    Unrecognised(String),
}

impl Classification {
    fn of(raw: &str) -> Self {
        match raw {
            "simple" => Self::Single,
            "twostage" => Self::Split,
            "tilting" => Self::TiltingVat,
            other => Self::Unrecognised(other.to_owned()),
        }
    }

    /// Whether the platform crosses its travel in 2 stages, or `None`
    /// for a classification that does not answer the question.
    pub(crate) fn splits_travel(&self) -> Option<bool> {
        match self {
            Self::Single => Some(false),
            Self::Split => Some(true),
            Self::TiltingVat | Self::Unrecognised(_) => None,
        }
    }
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

/// The outcome of mapping a printer record: the domain profile, the
/// record's raw `imageAssetPath` (still relative to the source file, for
/// the caller to resolve) if any, and what a material needs to place its
/// settings, which is the source's own id for the record and the
/// container token it names, both as written. The last 2 fields are what
/// the source claims about the machine that no profile of ours records.
pub(crate) struct MappedPrinter {
    pub profile: PrinterProfile,
    pub image_ref: Option<String>,
    pub preset_id: String,
    pub container: String,
    /// What the source says about how the machine separates a layer,
    /// which the profile deliberately does not carry.
    pub classification: Option<Classification>,
    /// How many bits the source says the panel drives, which no profile
    /// of ours records.
    pub panel_bits: Option<u32>,
}

/// Convert one DragonFruit printer record into a [`MappedPrinter`].
pub(crate) fn map_printer(raw: RawPrinter) -> Result<MappedPrinter, PrinterMapError> {
    let raw_id = raw.preset_id;

    let image_ref = raw.image_asset_path.as_ref().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    });

    let manufacturer = non_empty(&raw.manufacturer, &raw_id, "manufacturer")?;
    let name = non_empty(&raw.name, &raw_id, "name")?;
    let id = printer_id_of(manufacturer.as_str(), name.as_str()).map_err(|source| {
        PrinterMapError::InvalidId {
            id: format!("{manufacturer} {name}"),
            source,
        }
    })?;
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

    let container = raw.display.output_format;
    let output_binding = map_output_format(&container, raw.display.format_version)
        .ok_or_else(|| PrinterMapError::EmptyOutputFormat { id: raw_id.clone() })?;
    let capabilities = PrinterCapabilities {
        output_formats: NEVec::new(output_binding),
        supports_grayscale: raw.anti_aliasing,
        two_stage_lift: false,
        transition_layers: None,
        has_camera: raw.has_camera,
        network_protocol: map_network(raw.network_support.as_deref()),
    };

    let profile = PrinterProfile {
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
        defaults: PrinterDefaults::default(),
        time_calibration: PrinterTimeCalibration::default(),
    };
    Ok(MappedPrinter {
        profile,
        image_ref,
        preset_id: raw_id,
        container,
        classification: raw.display.settings_mode.as_deref().map(Classification::of),
        panel_bits: raw.bit_depth.map(|depth| depth.bits),
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

/// Resolve one axis to `(build_mm, pixel_um)`.
///
/// The build volume and resolution are what the slicer uses to place a
/// model on the raster, so when a build volume is given it is
/// authoritative and the pixel size is derived from it. This deliberately
/// ignores a stated pixel size: some source records fill it in wrongly,
/// for instance recording a square pixel on a panel whose pixels are not
/// square, and deriving keeps the pixel size consistent with the build
/// volume and resolution by construction. When the build volume is
/// absent the pixel size is trusted and used to derive the build volume
/// instead; when neither is given the axis cannot be determined.
fn resolve_axis(
    build_mm: Option<f64>,
    pixel_um: Option<f64>,
    resolution: u32,
) -> Option<(f64, f64)> {
    match (build_mm, pixel_um) {
        (Some(build), _) => Some((build, build / f64::from(resolution) * 1000.0)),
        (None, Some(pixel)) => Some((f64::from(resolution) * pixel / 1000.0, pixel)),
        (None, None) => None,
    }
}

/// Derive a printer's id from the machine it describes.
///
/// The id names a machine, so it is built from the machine's own
/// manufacturer and model and from nothing else. Source collections key
/// their presets however they like, container included; deriving here
/// keeps that out of our identities rather than editing it back out of
/// theirs.
///
/// A model whose name carries nothing a slug can hold yields an empty
/// id, which is refused.
fn printer_id_of(manufacturer: &str, name: &str) -> Result<PrinterId, InvalidProfileId> {
    let parts = format!("{manufacturer} {name}");
    let slug = parts
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-");
    PrinterId::new(&slug)
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
        "cxdlp" => FormatKind::Cxdlp,
        "azf" => FormatKind::Azf,
        "aff" => FormatKind::Aff,
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
        map_printer(serde_json::from_str(json).unwrap()).map(|mapped| mapped.profile)
    }

    #[test]
    fn captures_a_trimmed_image_reference() {
        let json = r#"{
            "presetId": "with-image", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "With Image",
            "imageAssetPath": "  ./assets/with-image.png  ",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let mapped = map_printer(serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(mapped.image_ref.as_deref(), Some("./assets/with-image.png"));
        assert_eq!(mapped.profile.identity.image_asset_path, None);
    }

    /// The source classifies the settings surface it shows for a
    /// machine, which is its word on how the platform separates a layer.
    #[test]
    fn the_classification_the_source_gives_is_read_and_not_written() {
        let record = |mode: &str| {
            format!(
                r#"{{
                    "presetId": "classified", "profileVersion": 1,
                    "manufacturer": "TestCorp", "family": "Series", "name": "Classified",
                    "antiAliasing": true,
                    "buildVolumeMm": {{ "width": 100.0, "depth": 100.0, "height": 40.0 }},
                    "pixelSize": {{ "x": 100.0, "y": 100.0 }},
                    "display": {{
                        "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo",
                        "settingsMode": "{mode}"
                    }}
                }}"#
            )
        };
        let mapped =
            |mode: &str| map_printer(serde_json::from_str(&record(mode)).unwrap()).unwrap();

        assert_eq!(
            mapped("simple").classification,
            Some(Classification::Single)
        );
        assert_eq!(
            mapped("twostage").classification,
            Some(Classification::Split)
        );
        assert_eq!(
            mapped("tilting").classification,
            Some(Classification::TiltingVat),
        );
        // Whatever the source says, the capability stays a curator's to
        // set from the manufacturer's own material.
        assert!(!mapped("twostage").profile.capabilities.two_stage_lift);
    }

    #[test]
    fn a_classification_this_converter_does_not_know_survives_as_the_word_it_is() {
        let json = r#"{
            "presetId": "future", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Future",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": {
                "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo",
                "settingsMode": "peristaltic"
            }
        }"#;
        let mapped = map_printer(serde_json::from_str(json).unwrap()).unwrap();

        assert_eq!(
            mapped.classification,
            Some(Classification::Unrecognised("peristaltic".to_owned())),
        );
    }

    #[test]
    fn a_record_that_states_no_classification_gives_none() {
        let json = r#"{
            "presetId": "plain", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Plain",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let mapped = map_printer(serde_json::from_str(json).unwrap()).unwrap();

        assert_eq!(mapped.classification, None);
        assert_eq!(mapped.panel_bits, None);
    }

    /// The panel's bit depth is a record's own key, next to the
    /// anti-aliasing flag our grayscale capability comes from.
    #[test]
    fn the_panels_bit_depth_is_read_and_the_grayscale_flag_still_comes_from_anti_aliasing() {
        let json = r#"{
            "presetId": "shallow", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Shallow",
            "antiAliasing": true,
            "bitDepth": { "bits": 3, "description": "3-bit color depth, suboptimal for grayscale" },
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let mapped = map_printer(serde_json::from_str(json).unwrap()).unwrap();

        assert_eq!(mapped.panel_bits, Some(3));
        assert!(mapped.profile.capabilities.supports_grayscale);
    }

    #[test]
    fn the_id_names_the_machine_whatever_the_source_keyed_its_preset_by() {
        let json = r#"{
            "presetId": "some-vendor-key-goo", "profileVersion": 1,
            "manufacturer": "ELEGOO", "family": "Saturn", "name": "Saturn 4",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let mapped = map_printer(serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(mapped.profile.identity.id.as_str(), "elegoo-saturn-4");
    }

    #[test]
    fn one_machine_offered_in_two_containers_derives_one_id() {
        let record = |format: &str| {
            format!(
                r#"{{
                    "presetId": "elegoo-saturn-4-{format}", "profileVersion": 1,
                    "manufacturer": "ELEGOO", "family": "Saturn", "name": "Saturn 4",
                    "buildVolumeMm": {{ "width": 100.0, "depth": 100.0, "height": 40.0 }},
                    "pixelSize": {{ "x": 100.0, "y": 100.0 }},
                    "display": {{
                        "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".{format}"
                    }}
                }}"#
            )
        };
        let goo = map_printer(serde_json::from_str(&record("goo")).unwrap()).unwrap();
        let ctb = map_printer(serde_json::from_str(&record("ctb")).unwrap()).unwrap();

        assert_eq!(goo.profile.identity.id, ctb.profile.identity.id);
    }

    #[test]
    fn a_model_name_slugifies_down_to_what_an_id_may_hold() {
        let json = r#"{
            "presetId": "x", "profileVersion": 1,
            "manufacturer": "Anycubic", "family": "Photon", "name": "Photon Mono X 6K/M3 Plus",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".pwmb" }
        }"#;
        let mapped = map_printer(serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(
            mapped.profile.identity.id.as_str(),
            "anycubic-photon-mono-x-6k-m3-plus",
        );
    }

    #[test]
    fn a_model_name_holding_nothing_an_id_can_carry_is_refused() {
        let json = r#"{
            "presetId": "x", "profileVersion": 1,
            "manufacturer": "\u30a8\u30ec\u30b0", "family": "S", "name": "\u30d7\u30ea\u30f3\u30bf",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let refused = map_printer(serde_json::from_str(json).unwrap());
        assert!(
            matches!(refused, Err(PrinterMapError::InvalidId { .. })),
            "a name with no slug in it must be refused",
        );
    }

    #[test]
    fn no_image_reference_when_absent_or_blank() {
        let json = r#"{
            "presetId": "no-image", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "No Image",
            "imageAssetPath": "   ",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        let mapped = map_printer(serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(mapped.image_ref, None);
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

        assert_eq!(
            profile.identity.id,
            PrinterId::new("testcorp-test-printer").unwrap(),
        );
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
    fn build_volume_wins_over_a_mis_recorded_square_pixel() {
        // A non-square panel whose source record wrongly lists a square
        // pixel: the build depth (5120 * 24um = 122.88) and resolution
        // are correct, but pixelSize.y was copied from pixelSize.x. The
        // build volume is authoritative, so the pixel size is derived
        // back to the real 19 x 24 and the envelope check passes.
        let json = r#"{
            "presetId": "nonsquare-panel", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Nonsquare Panel",
            "buildVolumeMm": { "width": 218.88, "depth": 122.88, "height": 200.0 },
            "pixelSize": { "x": 19.0, "y": 19.0 },
            "display": { "resolutionX": 11520, "resolutionY": 5120, "outputFormat": ".ctb" }
        }"#;
        let profile = map(json).unwrap();
        assert_eq!(
            profile.hardware.build_volume(),
            BuildVolume::new(218.88, 122.88, 200.0).unwrap(),
        );
        assert_eq!(
            profile.hardware.pixel_size_microns(),
            PixelSizeMicrons::new(19.0, 24.0).unwrap(),
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
            "presetId": "lumen-printer", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Lumen Printer",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".lumen" }
        }"#;
        let profile = map(json).unwrap();
        assert_eq!(
            *profile.capabilities.output_formats.first(),
            OutputFormatBinding {
                format: FormatKind::Unknown("lumen".to_owned()),
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
    fn a_source_key_our_ids_would_refuse_is_no_obstacle() {
        let json = r#"{
            "presetId": "Bad_Id", "profileVersion": 1,
            "manufacturer": "TestCorp", "family": "Series", "name": "Bad",
            "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
            "pixelSize": { "x": 100.0, "y": 100.0 },
            "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
        }"#;
        assert_eq!(map(json).unwrap().identity.id.as_str(), "testcorp-bad");
    }
}
