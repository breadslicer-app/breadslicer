/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! UVtools' printer catalog, as printer profiles.
//!
//! UVtools (AGPL-3.0, <https://github.com/sn4k3/UVtools>) bundles a
//! PrusaSlicer printer profile for every resin printer it converts files
//! for: `PrusaSlicer/printer/*.ini` in its source, and the same files
//! under `Assets/PrusaSlicer/printer` in an installation. Each states the
//! machine's panel (pixels and millimetres), its mirror, its build height,
//! and, in its notes, the file UVtools converts a slice to for it.
//!
//! The input is that folder, or some of its files. A printer becomes one
//! profile of ours:
//! - its panel, mirror and build height are UVtools';
//! - its file format is the one UVtools writes for it;
//! - what UVtools does not state (whether the machine splits its travel,
//!   transition layers, a camera, a photo) is left at "no".
//!
//! A printer we already ship keeps its id and everything UVtools does not
//! state. UVtools' panel, mirror and height replace ours where they
//! disagree, and its file format joins ours, first, when ours lacks it:
//! each change is reported. A profile checked against files that print on
//! the machine is kept as it is, and so is a printer UVtools does not list.
//!
//! The print values UVtools starts a converted file from (lift heights and
//! speeds, waits, light power) are not taken: they belong to a resin, and
//! our resin profiles carry them.

mod ini;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use nonempty_collections::NEVec;
use sc_print_primitives::{
    BuildVolume, FormatKind, Mirror, NonEmptyString, OutputFormatBinding, PixelResolution,
    PixelSizeMicrons, PrinterId,
};
use sc_print_profiles::{
    PrinterCapabilities, PrinterConstraints, PrinterDefaults, PrinterHardware, PrinterIdentity,
    PrinterProfile, PrinterTimeCalibration,
};
use semver::Version;

use crate::dragonfruit::printer::printer_id_of;
use crate::printer_library::PrinterLibrary;
use crate::profile::{ConvertedPrinter, ConvertedProfile};
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

use self::ini::IniFile;

/// The extension of the profiles UVtools bundles.
const PROFILE_EXTENSION: &str = "ini";

/// What starts the name of a profile UVtools wrote for a printer it does
/// not sell, as "UVtools Prusa SL1".
const UVTOOLS_OWN_PROFILE: &str = "UVtools ";

/// The printers whose profile was checked against files that print on the
/// machine. A run leaves them as they are, whatever UVtools states.
const PINNED: [&str; 1] = ["anycubic-photon-mono-m7-pro"];

/// The printers UVtools names differently from the profile we ship for the
/// machine: UVtools' name, and our id.
const SHIPPED_UNDER_ANOTHER_NAME: [(&str, &str); 3] = [
    // One machine sold under 2 names, as our profile's name says.
    (
        "Anycubic Photon M3 Plus",
        "anycubic-photon-mono-x-6k-m3-plus",
    ),
    (
        "Anycubic Photon Mono X 6K",
        "anycubic-photon-mono-x-6k-m3-plus",
    ),
    // ELEGOO sells it as the Saturn 4 Ultra; the 12K tells it from the 16K.
    ("Elegoo Saturn 4 Ultra 12K", "elegoo-saturn-4-ultra"),
];

/// The variants of Anycubic's binary file family, by the extension UVtools
/// names them with. The binding's version is the extension.
const ANYCUBIC_BINARY: [&str; 21] = [
    "pws", "pw0", "pwx", "dlp", "dl2p", "pwmx", "pmx2", "pwmb", "px6s", "pwmo", "pm3n", "pm4n",
    "pwms", "pwma", "pmsq", "pm3", "pm3m", "pm3r", "pm5", "pm5s", "m5sp",
];

/// The variants of Anycubic's ZIP file family, by extension.
const ANYCUBIC_ZIP: [&str; 6] = ["pwsz", "pm7", "pm7m", "pm4u", "pp1", "pp1m"];

/// The fewest decimals a length is compared to, in millimetres.
const MIN_DECIMALS: i32 = 2;

/// The CTB revision UVtools writes when a profile names none.
const CTB_DEFAULT_VERSION: u32 = 5;

/// The CXDLP revision UVtools writes when a profile names none.
const CXDLP_DEFAULT_VERSION: u32 = 3;

/// The GOO revision UVtools writes when a profile names none, as UVtools
/// numbers it (`30` is `V3.0`).
const GOO_DEFAULT_VERSION: u32 = 30;

/// Reads UVtools' printer catalog, consulting the printers we ship.
#[derive(Clone, Debug)]
pub struct UvtoolsCatalogReader {
    library: Option<PrinterLibrary>,
}

impl UvtoolsCatalogReader {
    /// A reader that merges UVtools' printers into the profiles `library`
    /// holds.
    pub fn consulting(library: PrinterLibrary) -> Self {
        Self {
            library: Some(library),
        }
    }

    /// A reader with no library to merge into. It refuses to read, since
    /// what it wrote would replace the printers we ship with UVtools'
    /// figures alone.
    pub fn without_a_library() -> Self {
        Self { library: None }
    }
}

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum UvtoolsCatalogError {
    /// No printer library was given, so a printer we ship would lose what
    /// UVtools does not state.
    NoPrinterLibrary,
    /// An input folder could not be listed.
    ReadDir { path: PathBuf, source: io::Error },
    /// A profile could not be read.
    Io { path: PathBuf, source: io::Error },
    /// The inputs hold no profile.
    NoProfiles,
    /// A profile states no value for a key every printer needs.
    MissingValue { path: PathBuf, key: &'static str },
    /// A profile states a value this reader cannot use.
    BadValue {
        path: PathBuf,
        key: &'static str,
        value: String,
    },
    /// A profile's name holds no manufacturer and model an id can be made
    /// from.
    UnnamedPrinter { path: PathBuf },
}

impl fmt::Display for UvtoolsCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPrinterLibrary => f.write_str(
                "the uvtools format needs --printer-library, so the printers we ship keep what UVtools does not state",
            ),
            Self::ReadDir { path, source } => {
                write!(f, "failed to list {}: {source}", path.display())
            }
            Self::Io { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::NoProfiles => f.write_str(
                "the inputs hold no UVtools printer profile (*.ini); pass UVtools' PrusaSlicer/printer folder",
            ),
            Self::MissingValue { path, key } => {
                write!(f, "{} states no {key}", path.display())
            }
            Self::BadValue { path, key, value } => {
                write!(f, "{} states {key} = {value:?}, which is not usable", path.display())
            }
            Self::UnnamedPrinter { path } => write!(
                f,
                "the name of {} holds no manufacturer and model to make an id from",
                path.display(),
            ),
        }
    }
}

impl std::error::Error for UvtoolsCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadDir { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl ProfileReader for UvtoolsCatalogReader {
    type Error = UvtoolsCatalogError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let library = self
            .library
            .as_ref()
            .ok_or(UvtoolsCatalogError::NoPrinterLibrary)?;
        let files = profile_files(inputs)?;
        if files.is_empty() {
            return Err(UvtoolsCatalogError::NoProfiles);
        }
        let mut run = Run::new(library);
        for path in files {
            let text =
                std::fs::read_to_string(&path).map_err(|source| UvtoolsCatalogError::Io {
                    path: path.clone(),
                    source,
                })?;
            let entry = CatalogEntry::from_ini(&path, &IniFile::parse(&text))?;
            run.absorb(&path, entry)?;
        }
        Ok(run.finish())
    }
}

/// Every profile the inputs name, in name order: a file as it is, a folder
/// by the profiles directly in it.
fn profile_files(inputs: &[&Path]) -> Result<Vec<PathBuf>, UvtoolsCatalogError> {
    let mut files = Vec::new();
    for input in inputs {
        if !input.is_dir() {
            files.push(input.to_path_buf());
            continue;
        }
        let entries = std::fs::read_dir(input).map_err(|source| UvtoolsCatalogError::ReadDir {
            path: input.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let path = entry
                .map_err(|source| UvtoolsCatalogError::ReadDir {
                    path: input.to_path_buf(),
                    source,
                })?
                .path();
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(PROFILE_EXTENSION))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// A length as UVtools writes it: its value, and how many decimals it was
/// written with.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Figure {
    value: f64,
    decimals: i32,
}

impl Figure {
    fn parse(text: &str) -> Option<Self> {
        let value: f64 = text.parse().ok()?;
        let decimals = text
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        (value.is_finite() && value > 0.0).then_some(Self {
            value,
            decimals: i32::try_from(decimals).ok()?,
        })
    }

    /// Report whether `ours` is this figure once written with as many
    /// decimals as UVtools wrote it with: 223.6416 mm is what "223.642"
    /// states. A figure written with fewer than 2 decimals is read to the
    /// hundredth, so "250" does not state 249.7.
    fn states(self, ours: f64) -> bool {
        let scale = 10f64.powi(self.decimals.max(MIN_DECIMALS));
        (ours * scale).round() == (self.value * scale).round()
    }
}

/// One printer of UVtools' catalog, as its profile states it.
#[derive(Clone, Debug, PartialEq)]
struct CatalogEntry {
    /// The profile's file name without its extension: the name UVtools
    /// gives the printer, as "Elegoo Saturn 4".
    title: String,
    width: Figure,
    depth: Figure,
    pixels_x: u32,
    pixels_y: u32,
    /// How the panel shows the file's image, as the file stores it.
    mirror: Mirror,
    height_mm: f64,
    /// Whether the panel lies as the file's image does. PrusaSlicer turns a
    /// portrait panel's image a quarter turn, which no profile of ours
    /// describes yet.
    landscape: bool,
    /// `FILEFORMAT_<extension>`: the file UVtools converts a slice to.
    file_format: Option<String>,
    /// `FILEVERSION_<n>`: the revision of it, when not UVtools' default.
    file_version: Option<u32>,
    /// `FILECLASS_<class>`: which of 2 formats sharing an extension.
    file_class: Option<String>,
    /// PrusaSlicer's model, which names the archive a profile without a
    /// file format writes.
    model: Option<String>,
}

impl CatalogEntry {
    fn from_ini(path: &Path, ini: &IniFile) -> Result<Self, UvtoolsCatalogError> {
        let value = |key: &'static str| {
            ini.get(key)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| UvtoolsCatalogError::MissingValue {
                    path: path.to_path_buf(),
                    key,
                })
        };
        let bad = |key: &'static str, value: &str| UvtoolsCatalogError::BadValue {
            path: path.to_path_buf(),
            key,
            value: value.to_owned(),
        };
        let figure = |key: &'static str| {
            let text = value(key)?;
            Figure::parse(text).ok_or_else(|| bad(key, text))
        };
        let count = |key: &'static str| {
            let text = value(key)?;
            text.parse::<u32>()
                .ok()
                .filter(|count| *count > 0)
                .ok_or_else(|| bad(key, text))
        };
        let flag = |key: &'static str| match value(key)? {
            "1" => Ok(true),
            "0" => Ok(false),
            other => Err(bad(key, other)),
        };
        let file_version = ini
            .keyword("FILEVERSION")
            .map(|text| text.parse::<u32>().map_err(|_| bad("FILEVERSION", text)))
            .transpose()?;
        let title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| UvtoolsCatalogError::UnnamedPrinter {
                path: path.to_path_buf(),
            })?
            .to_owned();
        Ok(Self {
            title,
            width: figure("display_width")?,
            depth: figure("display_height")?,
            pixels_x: count("display_pixels_x")?,
            pixels_y: count("display_pixels_y")?,
            mirror: Mirror {
                x: flag("display_mirror_x")?,
                y: flag("display_mirror_y")?,
            },
            height_mm: figure("max_print_height")?.value,
            landscape: value("display_orientation")? == "landscape",
            file_format: ini.keyword("FILEFORMAT").map(str::to_owned),
            file_version,
            file_class: ini.keyword("FILECLASS").map(str::to_owned),
            model: ini.get("printer_model").map(str::to_owned),
        })
    }

    /// The manufacturer and the model the entry's name gives, as "Elegoo"
    /// and "Saturn 4". A profile UVtools wrote for someone else's printer
    /// drops UVtools' name.
    fn brand_and_model(&self) -> Option<(&str, &str)> {
        let title = self
            .title
            .strip_prefix(UVTOOLS_OWN_PROFILE)
            .unwrap_or(&self.title);
        title
            .split_once(' ')
            .map(|(brand, model)| (brand.trim(), model.trim()))
            .filter(|(brand, model)| !brand.is_empty() && !model.is_empty())
    }

    /// The panel and plate UVtools states, mirrored as it states.
    fn hardware(&self, path: &Path) -> Result<PrinterHardware, UvtoolsCatalogError> {
        let bad = |key: &'static str, value: String| UvtoolsCatalogError::BadValue {
            path: path.to_path_buf(),
            key,
            value,
        };
        let resolution = PixelResolution::new(self.pixels_x, self.pixels_y).map_err(|_| {
            bad(
                "display_pixels_x",
                format!("{} x {}", self.pixels_x, self.pixels_y),
            )
        })?;
        let volume = BuildVolume::new(self.width.value, self.depth.value, self.height_mm)
            .map_err(|_| bad("max_print_height", self.height_mm.to_string()))?;
        // Multiplying first keeps the figures UVtools rounds to whole
        // microns whole: 68.04 mm over 1440 pixels is 47.25 µm.
        let pixel = PixelSizeMicrons::new(
            self.width.value * 1000.0 / f64::from(self.pixels_x),
            self.depth.value * 1000.0 / f64::from(self.pixels_y),
        )
        .map_err(|_| bad("display_width", self.width.value.to_string()))?;
        PrinterHardware::new(volume, resolution, pixel, self.mirror)
            .map_err(|_| bad("display_width", self.width.value.to_string()))
    }

    /// The file UVtools converts a slice to for this printer.
    fn binding(&self) -> OutputFormatBinding {
        let Some(token) = self.file_format.as_deref() else {
            // No file format means PrusaSlicer's own archive, whose
            // extension the model names (`.sl1`, `.sl1s`).
            let model = self.model.as_deref().unwrap_or("sl1");
            return binding(FormatKind::Sl1, Some(model.to_ascii_lowercase()));
        };
        let extension = token.to_ascii_lowercase();
        let version = self.file_version;
        match extension.as_str() {
            "ctb" => binding(
                FormatKind::Ctb,
                Some(format!("v{}", version.unwrap_or(CTB_DEFAULT_VERSION))),
            ),
            "encrypted.ctb" => binding(
                FormatKind::Ctb,
                Some(format!("v{}enc", version.unwrap_or(CTB_DEFAULT_VERSION))),
            ),
            "goo" => {
                let version = version.unwrap_or(GOO_DEFAULT_VERSION);
                binding(
                    FormatKind::Goo,
                    Some(format!("v{}.{}", version / 10, version % 10)),
                )
            }
            "cxdlp" => binding(
                FormatKind::Cxdlp,
                Some(format!("v{}", version.unwrap_or(CXDLP_DEFAULT_VERSION))),
            ),
            other if ANYCUBIC_BINARY.contains(&other) => {
                binding(FormatKind::Aff, Some(other.to_owned()))
            }
            other if ANYCUBIC_ZIP.contains(&other) => {
                binding(FormatKind::Azf, Some(other.to_owned()))
            }
            other => {
                let version = self
                    .file_class
                    .as_deref()
                    .map(str::to_ascii_lowercase)
                    .or_else(|| version.map(|version| format!("v{version}")));
                binding(FormatKind::Unknown(other.to_owned()), version)
            }
        }
    }
}

fn binding(format: FormatKind, version: Option<String>) -> OutputFormatBinding {
    OutputFormatBinding {
        format,
        version: version.and_then(|version| NonEmptyString::new(&version).ok()),
    }
}

/// What UVtools states about one machine, in our terms, with the plate's
/// lengths as UVtools wrote them.
#[derive(Clone, Debug, PartialEq)]
struct Catalogued {
    hardware: PrinterHardware,
    binding: OutputFormatBinding,
    width: Figure,
    depth: Figure,
}

/// One read's progress.
#[derive(Debug)]
struct Run<'a> {
    library: &'a PrinterLibrary,
    /// The machines read so far, with the name of the entry that stated them.
    seen: BTreeMap<PrinterId, (String, Catalogued)>,
    profiles: Vec<ConvertedProfile>,
    notes: Vec<SourceNote>,
}

impl<'a> Run<'a> {
    fn new(library: &'a PrinterLibrary) -> Self {
        Self {
            library,
            seen: BTreeMap::new(),
            profiles: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn absorb(&mut self, path: &Path, entry: CatalogEntry) -> Result<(), UvtoolsCatalogError> {
        if !entry.landscape {
            self.notes.push(SourceNote::PortraitPanelSkipped {
                uvtools: entry.title,
            });
            return Ok(());
        }
        let unnamed = || UvtoolsCatalogError::UnnamedPrinter {
            path: path.to_path_buf(),
        };
        let (brand, model) = entry.brand_and_model().ok_or_else(unnamed)?;
        let manufacturer = self.manufacturer_spelling(brand);
        let name = model_name(brand, model);
        let id = match self.shipped_under_another_name(&entry.title) {
            Some(id) => id,
            None => printer_id_of(&manufacturer, &name).map_err(|_| unnamed())?,
        };
        let catalogued = Catalogued {
            hardware: entry.hardware(path)?,
            binding: entry.binding(),
            width: entry.width,
            depth: entry.depth,
        };

        if let Some((first, stated)) = self.seen.get(&id) {
            self.notes.push(if *stated == catalogued {
                SourceNote::UvtoolsPrintersShareAProfile {
                    printer: id,
                    first: first.clone(),
                    second: entry.title,
                }
            } else {
                SourceNote::UvtoolsPrintersDisagreeOnAProfile {
                    printer: id,
                    kept: first.clone(),
                    passed_over: entry.title,
                }
            });
            return Ok(());
        }
        self.seen
            .insert(id.clone(), (entry.title.clone(), catalogued.clone()));
        if matches!(catalogued.binding.format, FormatKind::Unknown(_)) {
            self.notes.push(SourceNote::FormatUnknownHere {
                printer: id.clone(),
                format: catalogued.binding.to_string(),
            });
        }

        let profile = match self.library.profile(&id) {
            Some(_) if PINNED.contains(&id.as_str()) => {
                self.notes
                    .push(SourceNote::PinnedProfileKept { printer: id });
                return Ok(());
            }
            Some(ours) => {
                let merged = merged(ours, &catalogued, &mut self.notes);
                if merged == *ours {
                    return Ok(());
                }
                merged
            }
            None => {
                let manufacturer = NonEmptyString::new(&manufacturer).map_err(|_| unnamed())?;
                let family = NonEmptyString::new(family_of(&name)).map_err(|_| unnamed())?;
                let name = NonEmptyString::new(&name).map_err(|_| unnamed())?;
                new_profile(id, manufacturer, family, name, catalogued)
            }
        };
        self.profiles
            .push(ConvertedProfile::Printer(ConvertedPrinter {
                profile,
                image_source: None,
            }));
        Ok(())
    }

    /// The manufacturer as the printers we ship already spell it, so a
    /// new machine sits beside its siblings ("ELEGOO", not "Elegoo").
    fn manufacturer_spelling(&self, brand: &str) -> String {
        self.library
            .profiles()
            .map(|profile| profile.identity.manufacturer.as_str())
            .find(|ours| ours.eq_ignore_ascii_case(brand))
            .unwrap_or(brand)
            .to_owned()
    }

    /// Our id for a machine UVtools names differently, when we ship it.
    fn shipped_under_another_name(&self, title: &str) -> Option<PrinterId> {
        SHIPPED_UNDER_ANOTHER_NAME
            .iter()
            .find(|(uvtools, _)| *uvtools == title)
            .and_then(|(_, id)| PrinterId::new(id).ok())
            .filter(|id| self.library.profile(id).is_some())
    }

    fn finish(mut self) -> SourceReading {
        for profile in self.library.profiles() {
            let id = &profile.identity.id;
            if !self.seen.contains_key(id) {
                self.notes.push(SourceNote::PrinterUvtoolsLacks {
                    printer: id.clone(),
                });
            }
        }
        SourceReading {
            profiles: self.profiles,
            notes: self.notes,
        }
    }
}

/// Our profile for a machine UVtools also lists, with UVtools' panel,
/// mirror, height and file format where ours disagrees.
fn merged(
    ours: &PrinterProfile,
    theirs: &Catalogued,
    notes: &mut Vec<SourceNote>,
) -> PrinterProfile {
    let printer = ours.identity.id.clone();
    let hardware = ours.hardware;
    let stated = theirs.hardware;
    let swaps = hardware.panel_rotation().swaps_axes();

    // UVtools describes the image as the file stores it; a profile that
    // turns its panel describes the plate before the turn.
    let (pixels, (x_mm, y_mm)) = stored_panel(&hardware);
    let agrees = pixels == (stated.pixel_resolution().x(), stated.pixel_resolution().y())
        && theirs.width.states(x_mm)
        && theirs.depth.states(y_mm);
    let mirror = if swaps {
        Mirror {
            x: stated.mirror().y,
            y: stated.mirror().x,
        }
    } else {
        stated.mirror()
    };
    let height = stated.build_volume().z();

    let hardware_now = if agrees {
        let volume = BuildVolume::new(
            hardware.build_volume().x_mm(),
            hardware.build_volume().y_mm(),
            height.value(),
        )
        .expect("a plate we ship and a height UVtools states are both valid");
        PrinterHardware::new(
            volume,
            hardware.pixel_resolution(),
            hardware.pixel_size_microns(),
            mirror,
        )
        .expect("the plate we ship already agrees with its own pixels")
        .with_panel_rotation(hardware.panel_rotation())
    } else {
        notes.push(SourceNote::PanelFollowsUvtools {
            printer: printer.clone(),
            ours: describe_panel(&hardware),
            uvtools: describe_panel(&stated),
        });
        stated
    };
    if hardware_now.mirror() != hardware.mirror() {
        notes.push(SourceNote::MirrorFollowsUvtools {
            printer: printer.clone(),
            ours: describe_mirror(hardware.mirror()),
            uvtools: describe_mirror(hardware_now.mirror()),
        });
    }
    if hardware_now.build_volume().z_mm() != hardware.build_volume().z_mm() {
        notes.push(SourceNote::HeightFollowsUvtools {
            printer: printer.clone(),
            ours: format!("{} mm", hardware.build_volume().z_mm()),
            uvtools: format!("{} mm", hardware_now.build_volume().z_mm()),
        });
    }

    let declared = &ours.capabilities.output_formats;
    let output_formats = if declared.iter().any(|binding| *binding == theirs.binding) {
        declared.clone()
    } else {
        notes.push(SourceNote::FormatAddedFromUvtools {
            printer,
            format: theirs.binding.to_string(),
        });
        let mut formats = NEVec::new(theirs.binding.clone());
        formats.extend(declared.iter().cloned());
        formats
    };

    PrinterProfile {
        hardware: hardware_now,
        capabilities: PrinterCapabilities {
            output_formats,
            ..ours.capabilities.clone()
        },
        ..ours.clone()
    }
}

/// A profile's panel in pixels and its plate in millimetres, as its files
/// store them.
fn stored_panel(hardware: &PrinterHardware) -> ((u32, u32), (f64, f64)) {
    let pixels = hardware.pixel_resolution();
    let plate = hardware.build_volume();
    if hardware.panel_rotation().swaps_axes() {
        ((pixels.y(), pixels.x()), (plate.y_mm(), plate.x_mm()))
    } else {
        ((pixels.x(), pixels.y()), (plate.x_mm(), plate.y_mm()))
    }
}

fn describe_panel(hardware: &PrinterHardware) -> String {
    let pixels = hardware.pixel_resolution();
    let plate = hardware.build_volume();
    format!(
        "{} x {} px, {} x {} mm",
        pixels.x(),
        pixels.y(),
        plate.x_mm(),
        plate.y_mm(),
    )
}

fn describe_mirror(mirror: Mirror) -> String {
    match (mirror.x, mirror.y) {
        (false, false) => "no mirror",
        (true, false) => "mirror X",
        (false, true) => "mirror Y",
        (true, true) => "mirror X and Y",
    }
    .to_owned()
}

/// A profile for a machine we do not ship yet.
fn new_profile(
    id: PrinterId,
    manufacturer: NonEmptyString,
    family: NonEmptyString,
    name: NonEmptyString,
    catalogued: Catalogued,
) -> PrinterProfile {
    PrinterProfile {
        identity: PrinterIdentity {
            id,
            manufacturer,
            family,
            name,
            profile_version: Version::new(1, 0, 0),
            image_asset_path: None,
        },
        hardware: catalogued.hardware,
        capabilities: PrinterCapabilities {
            output_formats: NEVec::new(catalogued.binding),
            // Every LCD panel UVtools writes for shows grays; the flag only
            // describes the machine.
            supports_grayscale: true,
            // UVtools states neither, so a resin's second stage is dropped
            // with a warning rather than sent to a machine that may not run
            // it.
            two_stage_lift: false,
            transition_layers: None,
            has_camera: false,
            network_protocol: None,
        },
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
    }
}

/// The machine's name as a profile of ours carries it.
///
/// UVtools appends Creality's model code to most Halot names ("Halot One
/// CL-60"), which our Halot profiles leave out, and tells 2 profiles of one
/// machine apart with a " - " suffix ("Proxima 6 - SVGX"), which reads as a
/// parenthesis.
fn model_name(brand: &str, model: &str) -> String {
    let model = if brand.eq_ignore_ascii_case("Creality") {
        without_model_code(model)
    } else {
        model
    };
    match model.split_once(" - ") {
        Some((machine, variant)) => format!("{machine} ({variant})"),
        None => model.to_owned(),
    }
}

fn without_model_code(model: &str) -> &str {
    match model.rsplit_once(' ') {
        Some((machine, code)) if is_model_code(code) => machine,
        _ => model,
    }
}

/// Report whether a word is a Creality model code, as "CL-60" or "CL925".
fn is_model_code(word: &str) -> bool {
    word.strip_prefix("CL")
        .map(|rest| rest.trim_start_matches('-'))
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// The series a model belongs to: the letters its name opens with, as
/// "Sonic" for "Sonic Mini 8K" or "Bene" for "Bene4 Mono". A model whose
/// first word opens with a single letter, as "X1" or "N4", is a series of
/// that word.
fn family_of(model: &str) -> &str {
    let first = model.split_whitespace().next().unwrap_or(model);
    let letters = first.len()
        - first
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .len();
    if letters >= 2 {
        &first[..letters]
    } else {
        first
    }
}

#[cfg(test)]
mod tests;
