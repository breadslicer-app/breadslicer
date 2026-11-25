/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A printer added by hand (#600): the form's fields as text, read back into a
//! printer profile for the user library.
//!
//! The panel is described by its resolution in pixels and either its pixel
//! size or its plate size: given one, the other follows; given both, they have
//! to agree, as a bundled profile's do. Everything the form does not ask for
//! (constraints, the bottom layer count a machine expects) comes from the
//! printer the user starts from, when there is one.

use std::collections::BTreeSet;
use std::fmt;

use nonempty_collections::NEVec;
use sc_print_primitives::{
    BuildVolume, Mirror, NonEmptyString, PanelRotation, PixelResolution, PixelSizeMicrons,
    PrinterId,
};
use sc_print_profiles::{
    PrinterCapabilities, PrinterConstraints, PrinterHardware, PrinterIdentity, PrinterProfile,
};
use sc_sla_formats::{
    AnycubicBinaryVariant, AnycubicZipVariant, CtbVariant, CxdlpVersion, GooVersion, OtherFormat,
    PrinterFileFormat, Sl1Variant,
};
use semver::Version;

use crate::i18n::ctr;

/// The largest panel the form accepts on an axis, in pixels.
const MAX_PIXELS: u32 = 40_000;
/// The pixel sizes the form accepts, in micrometres.
const PIXEL_UM: (f64, f64) = (1.0, 500.0);
/// The plate and height sizes the form accepts, in millimetres.
const PLATE_MM: (f64, f64) = (1.0, 2_000.0);
/// How far a stated plate may differ from the pixels it is made of, as the
/// bundled profiles allow.
const PLATE_TOLERANCE: f64 = 0.005;

/// What the form holds, as typed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PrinterDraft {
    pub manufacturer: String,
    pub name: String,
    pub resolution_x: String,
    pub resolution_y: String,
    pub pixel_x_um: String,
    pub pixel_y_um: String,
    pub plate_x_mm: String,
    pub plate_y_mm: String,
    pub height_mm: String,
    pub mirror_x: bool,
    pub mirror_y: bool,
    /// The file stores the plate turned a quarter (the Halot One's panel).
    pub turned: bool,
    pub grayscale: bool,
    pub format: Option<PrinterFileFormat>,
}

/// A field of the form, to point a message at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrinterField {
    Manufacturer,
    Name,
    ResolutionX,
    ResolutionY,
    PixelX,
    PixelY,
    PlateX,
    PlateY,
    Height,
    Format,
}

/// Why the form cannot become a printer yet.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PrinterFormError {
    Missing(PrinterField),
    NotANumber(PrinterField),
    OutOfRange(PrinterField),
    /// Neither the pixel size nor the plate size is given on an axis.
    NoSize(PrinterField),
    /// The plate stated on an axis is not what its pixels make.
    PlateDisagrees {
        field: PrinterField,
        stated_mm: f64,
        pixels_mm: f64,
    },
    /// The name is another printer's already.
    NameTaken,
}

impl PrinterFormError {
    /// The field the message is about.
    #[cfg(test)]
    pub(crate) fn field(&self) -> Option<PrinterField> {
        match self {
            Self::Missing(f) | Self::NotANumber(f) | Self::OutOfRange(f) | Self::NoSize(f) => {
                Some(*f)
            }
            Self::PlateDisagrees { field, .. } => Some(*field),
            Self::NameTaken => Some(PrinterField::Name),
        }
    }
}

impl fmt::Display for PrinterFormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Missing(field) => {
                ctr!("PrinterDialog", "{} is missing.", field_name(*field))
            }
            Self::NotANumber(field) => {
                ctr!("PrinterDialog", "{} is not a number.", field_name(*field))
            }
            Self::OutOfRange(field) => ctr!(
                "PrinterDialog",
                "{} is out of range ({}).",
                field_name(*field),
                range_of(*field)
            ),
            Self::NoSize(field) => ctr!(
                "PrinterDialog",
                "Give the pixel size or the plate size for {}.",
                field_name(*field)
            ),
            Self::PlateDisagrees {
                field,
                stated_mm,
                pixels_mm,
            } => ctr!(
                "PrinterDialog",
                "{} is {} mm, but the pixels make {} mm. Leave one of the two empty.",
                field_name(*field),
                trim_number(*stated_mm),
                trim_number(*pixels_mm)
            ),
            Self::NameTaken => ctr!(
                "PrinterDialog",
                "A printer with this manufacturer and name already exists."
            ),
        };
        f.write_str(&text)
    }
}

fn field_name(field: PrinterField) -> String {
    match field {
        PrinterField::Manufacturer => ctr!("PrinterDialog", "Manufacturer"),
        PrinterField::Name => ctr!("PrinterDialog", "Model name"),
        PrinterField::ResolutionX => ctr!("PrinterDialog", "Resolution X"),
        PrinterField::ResolutionY => ctr!("PrinterDialog", "Resolution Y"),
        PrinterField::PixelX => ctr!("PrinterDialog", "Pixel size X"),
        PrinterField::PixelY => ctr!("PrinterDialog", "Pixel size Y"),
        PrinterField::PlateX => ctr!("PrinterDialog", "Plate X"),
        PrinterField::PlateY => ctr!("PrinterDialog", "Plate Y"),
        PrinterField::Height => ctr!("PrinterDialog", "Build height"),
        PrinterField::Format => ctr!("PrinterDialog", "File format"),
    }
    .to_string()
}

fn range_of(field: PrinterField) -> String {
    match field {
        PrinterField::ResolutionX | PrinterField::ResolutionY => {
            format!("1–{MAX_PIXELS} px")
        }
        PrinterField::PixelX | PrinterField::PixelY => {
            format!("{}–{} µm", PIXEL_UM.0, PIXEL_UM.1)
        }
        _ => format!("{}–{} mm", PLATE_MM.0, PLATE_MM.1),
    }
}

/// A number as a person would write it: no trailing zeros.
pub(crate) fn trim_number(value: f64) -> String {
    let text = format!("{:.4}", value);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        crate::numbers::shown(text)
    }
}

/// Reads a number, taking a comma as the decimal mark too.
fn number(field: PrinterField, text: &str) -> Result<Option<f64>, PrinterFormError> {
    let text = crate::numbers::normal(text);
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Some)
        .ok_or(PrinterFormError::NotANumber(field))
}

fn in_range(
    field: PrinterField,
    value: f64,
    (low, high): (f64, f64),
) -> Result<f64, PrinterFormError> {
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(PrinterFormError::OutOfRange(field))
    }
}

fn pixels(field: PrinterField, text: &str) -> Result<u32, PrinterFormError> {
    let value = number(field, text)?.ok_or(PrinterFormError::Missing(field))?;
    if value.fract() != 0.0 {
        return Err(PrinterFormError::NotANumber(field));
    }
    in_range(field, value, (1.0, f64::from(MAX_PIXELS))).map(|v| v as u32)
}

/// One axis of the panel: its pixels, their size and the plate they make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Axis {
    pub pixels: u32,
    pub pixel_um: f64,
    pub plate_mm: f64,
}

fn axis(
    resolution: (PrinterField, &str),
    pixel: (PrinterField, &str),
    plate: (PrinterField, &str),
) -> Result<Axis, PrinterFormError> {
    let pixels = pixels(resolution.0, resolution.1)?;
    let pixel_um = number(pixel.0, pixel.1)?
        .map(|v| in_range(pixel.0, v, PIXEL_UM))
        .transpose()?;
    let plate_mm = number(plate.0, plate.1)?
        .map(|v| in_range(plate.0, v, PLATE_MM))
        .transpose()?;
    let count = f64::from(pixels);
    match (pixel_um, plate_mm) {
        (Some(um), None) => Ok(Axis {
            pixels,
            pixel_um: um,
            plate_mm: in_range(plate.0, count * um / 1000.0, PLATE_MM)?,
        }),
        (None, Some(mm)) => Ok(Axis {
            pixels,
            pixel_um: in_range(pixel.0, mm * 1000.0 / count, PIXEL_UM)?,
            plate_mm: mm,
        }),
        (Some(um), Some(mm)) => {
            let made = count * um / 1000.0;
            if ((mm - made) / made).abs() > PLATE_TOLERANCE {
                return Err(PrinterFormError::PlateDisagrees {
                    field: plate.0,
                    stated_mm: mm,
                    pixels_mm: made,
                });
            }
            Ok(Axis {
                pixels,
                pixel_um: um,
                plate_mm: mm,
            })
        }
        (None, None) => Err(PrinterFormError::NoSize(resolution.0)),
    }
}

/// What the panel fields make, for the form to show as they are typed: each
/// axis that reads, and the first error.
pub(crate) fn panel_of(draft: &PrinterDraft) -> (Option<Axis>, Option<Axis>) {
    let x = axis(
        (PrinterField::ResolutionX, &draft.resolution_x),
        (PrinterField::PixelX, &draft.pixel_x_um),
        (PrinterField::PlateX, &draft.plate_x_mm),
    )
    .ok();
    let y = axis(
        (PrinterField::ResolutionY, &draft.resolution_y),
        (PrinterField::PixelY, &draft.pixel_y_um),
        (PrinterField::PlateY, &draft.plate_y_mm),
    )
    .ok();
    (x, y)
}

/// A printer id from a manufacturer and a model name.
pub(crate) fn id_text(manufacturer: &str, name: &str) -> String {
    format!("{manufacturer} {name}")
        .to_ascii_lowercase()
        .replace('+', " plus ")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Reads the form into a printer profile.
///
/// `base` is the printer the form started from, whose fields the form does
/// not ask for are kept; `editing` is the id of the user printer being
/// changed, which keeps its id. `taken` holds every printer id that exists.
pub(crate) fn build_printer(
    draft: &PrinterDraft,
    base: Option<&PrinterProfile>,
    editing: Option<&PrinterId>,
    taken: &BTreeSet<PrinterId>,
) -> Result<PrinterProfile, PrinterFormError> {
    let manufacturer = NonEmptyString::new(draft.manufacturer.trim())
        .map_err(|_| PrinterFormError::Missing(PrinterField::Manufacturer))?;
    let name = NonEmptyString::new(draft.name.trim())
        .map_err(|_| PrinterFormError::Missing(PrinterField::Name))?;
    let id = match editing {
        Some(id) => id.clone(),
        None => {
            let text = id_text(manufacturer.as_str(), name.as_str());
            let id =
                PrinterId::new(&text).map_err(|_| PrinterFormError::Missing(PrinterField::Name))?;
            if taken.contains(&id) {
                return Err(PrinterFormError::NameTaken);
            }
            id
        }
    };
    let x = axis(
        (PrinterField::ResolutionX, &draft.resolution_x),
        (PrinterField::PixelX, &draft.pixel_x_um),
        (PrinterField::PlateX, &draft.plate_x_mm),
    )?;
    let y = axis(
        (PrinterField::ResolutionY, &draft.resolution_y),
        (PrinterField::PixelY, &draft.pixel_y_um),
        (PrinterField::PlateY, &draft.plate_y_mm),
    )?;
    let height = number(PrinterField::Height, &draft.height_mm)?
        .ok_or(PrinterFormError::Missing(PrinterField::Height))?;
    let height = in_range(PrinterField::Height, height, PLATE_MM)?;
    let format = draft
        .format
        .ok_or(PrinterFormError::Missing(PrinterField::Format))?;

    let volume = BuildVolume::new(x.plate_mm, y.plate_mm, height)
        .map_err(|_| PrinterFormError::OutOfRange(PrinterField::Height))?;
    let resolution = PixelResolution::new(x.pixels, y.pixels)
        .map_err(|_| PrinterFormError::OutOfRange(PrinterField::ResolutionX))?;
    let pixel = PixelSizeMicrons::new(x.pixel_um, y.pixel_um)
        .map_err(|_| PrinterFormError::OutOfRange(PrinterField::PixelX))?;
    let mirror = Mirror {
        x: draft.mirror_x,
        y: draft.mirror_y,
    };
    let hardware = PrinterHardware::new(volume, resolution, pixel, mirror)
        .map_err(|err| PrinterFormError::PlateDisagrees {
            field: match err.axis {
                sc_print_primitives::PixelAxis::X => PrinterField::PlateX,
                sc_print_primitives::PixelAxis::Y => PrinterField::PlateY,
            },
            stated_mm: err.build_volume_mm,
            pixels_mm: err.derived_mm,
        })?
        .with_panel_rotation(if draft.turned {
            PanelRotation::Ccw90
        } else {
            PanelRotation::None
        });

    // The printer started from, or the one being edited.
    let from_base = base;
    let capabilities = PrinterCapabilities {
        output_formats: NEVec::new(format.binding()),
        supports_grayscale: draft.grayscale,
        two_stage_lift: from_base.is_some_and(|b| b.capabilities.two_stage_lift),
        transition_layers: from_base.and_then(|b| b.capabilities.transition_layers),
        has_camera: false,
        tilting_vat: from_base.is_some_and(|b| b.capabilities.tilting_vat),
        tilts_bottom_layers: from_base.is_some_and(|b| b.capabilities.tilts_bottom_layers),
        network_protocol: None,
    };
    let family = base
        .filter(|b| editing == Some(&b.identity.id))
        .map(|b| b.identity.family.clone())
        .unwrap_or_else(|| name.clone());
    Ok(PrinterProfile {
        identity: PrinterIdentity {
            id,
            manufacturer,
            family,
            name,
            profile_version: Version::new(1, 0, 0),
            image_asset_path: base
                .filter(|b| editing == Some(&b.identity.id))
                .and_then(|b| b.identity.image_asset_path.clone()),
            // Where it came from (#20): the user's own printer changed keeps
            // its own; a copy names the printer it was made from.
            origin: match base {
                Some(b) if editing == Some(&b.identity.id) => b.identity.origin.clone(),
                Some(b) => Some(sc_print_profiles::ProfileOrigin::made_from(
                    &format!("{} {}", b.identity.manufacturer, b.identity.name),
                    b.identity.id.as_str(),
                    &crate::profile_origin::today(),
                )),
                None => None,
            },
        },
        hardware,
        capabilities,
        constraints: from_base
            .map(|b| b.constraints)
            .unwrap_or_else(no_constraints),
        defaults: from_base.map(|b| b.defaults).unwrap_or_default(),
        // A machine copied from has its own timing; this one has none yet,
        // unless it is the one being edited.
        time_calibration: base
            .filter(|b| editing == Some(&b.identity.id))
            .map(|b| b.time_calibration)
            .unwrap_or_default(),
    })
}

fn no_constraints() -> PrinterConstraints {
    toml::from_str("").expect("every constraint is optional")
}

/// The form filled from a printer: to start from it, or to edit it.
pub(crate) fn draft_of(profile: &PrinterProfile) -> PrinterDraft {
    let hw = &profile.hardware;
    let format = profile
        .capabilities
        .output_formats
        .iter()
        .find_map(|binding| PrinterFileFormat::writable_from_binding(binding).ok());
    PrinterDraft {
        manufacturer: profile.identity.manufacturer.as_str().to_owned(),
        name: profile.identity.name.as_str().to_owned(),
        resolution_x: hw.pixel_resolution().x().to_string(),
        resolution_y: hw.pixel_resolution().y().to_string(),
        pixel_x_um: exact(hw.pixel_size_microns().x.value()),
        pixel_y_um: exact(hw.pixel_size_microns().y.value()),
        plate_x_mm: exact(hw.build_volume().x_mm()),
        plate_y_mm: exact(hw.build_volume().y_mm()),
        height_mm: exact(hw.build_volume().z_mm()),
        mirror_x: hw.mirror().x,
        mirror_y: hw.mirror().y,
        turned: hw.panel_rotation() == PanelRotation::Ccw90,
        grayscale: profile.capabilities.supports_grayscale,
        format,
    }
}

/// A number exactly as stored, so a printer filled into the form and read
/// back is the same machine.
fn exact(value: f64) -> String {
    format!("{value}")
}

/// Every printer file this build writes, in the order the form offers them.
pub(crate) fn writable_formats() -> Vec<PrinterFileFormat> {
    let mut all = vec![
        PrinterFileFormat::Ctb(CtbVariant::EncryptedV5),
        PrinterFileFormat::Ctb(CtbVariant::PlainV5),
        PrinterFileFormat::Ctb(CtbVariant::V4),
        PrinterFileFormat::Ctb(CtbVariant::V3),
        PrinterFileFormat::Ctb(CtbVariant::V2),
        PrinterFileFormat::Goo(GooVersion::V3_0),
        PrinterFileFormat::Goo(GooVersion::V5_1),
        PrinterFileFormat::Cxdlp(CxdlpVersion::V3),
        PrinterFileFormat::Cxdlp(CxdlpVersion::V2),
    ];
    all.extend(AnycubicZipVariant::ALL.map(PrinterFileFormat::AnycubicZip));
    all.extend(AnycubicBinaryVariant::ALL.map(PrinterFileFormat::AnycubicBinary));
    all.extend(Sl1Variant::ALL.map(PrinterFileFormat::Sl1));
    all.extend(OtherFormat::ALL.map(PrinterFileFormat::Other));
    all.retain(|format| format.has_writer());
    all
}

#[cfg(test)]
mod tests;
